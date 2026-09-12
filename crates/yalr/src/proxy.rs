//! Proxy-Handler: fuehrt Requests gegen Provider aus (mit Retry/Fallback),
//! streamt SSE transparent weiter und loggt alles async.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{json, Value};
use uuid::Uuid;

use common::auth;
use common::state::AppState;
use ingest::{LiveEvent, LiveLog, RequestLog};
use providers::{compute_cost, estimate_tokens, ProviderError, ProviderKind, RouteTarget};

const MAX_RETRIES: usize = 2;
const RETRY_DELAY: Duration = Duration::from_millis(300);

pub struct ProxyOutcome {
    pub response: Response,
    pub log: RequestLog,
}

/// Behandelt `/v1/chat/completions`, `/v1/embeddings`, `/v1/messages` und `/v1/models`.
pub async fn proxy(
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let endpoint = endpoint_from_uri(&parts.uri);
    let body = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let started = Instant::now();
    let request_id = Uuid::new_v4().to_string();

    // 1) Auth
    let vk = match auth::authenticate(&state, &headers).await {
        Ok(vk) => vk,
        Err(status) => {
            return error_response(state, None, &request_id, &endpoint, started, status, "auth", &status_text(&status), &body).await;
        }
    };

    // 2) Request-Body parsen (model etc.)
    let req_json: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(state, Some(&vk), &request_id, &endpoint, started, StatusCode::BAD_REQUEST, "invalid_json", &e.to_string(), &body).await;
        }
    };

    let requested_model = req_json
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();

    if requested_model.is_empty() {
        return error_response(state, Some(&vk), &request_id, &endpoint, started, StatusCode::BAD_REQUEST, "missing_model", "request body has no 'model' field", &body).await;
    }

    // 3) Route aufloesen (hybrid: header-override gewinnt, sonst model-name)
    let provider_override = headers
        .get("x-llm-provider")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut targets = match state.resolve_route(&requested_model).await {
        Some(t) => t,
        None => {
            return error_response(state, Some(&vk), &request_id, &endpoint, started, StatusCode::NOT_FOUND, "unknown_model", &format!("model '{requested_model}' is not configured"), &body).await;
        }
    };

    // Provider-Override: nur targets des gewuenschten providers behalten
    if let Some(override_name) = &provider_override {
        let kind = ProviderKind::parse(override_name).ok();
        let before = targets.len();
        targets.retain(|t| {
            kind.map(|k| t.provider_kind == k).unwrap_or(false)
                || t.provider_name == *override_name
        });
        if targets.is_empty() {
            let msg = format!("no enabled provider '{override_name}' for model '{requested_model}'");
            return error_response(state, Some(&vk), &request_id, &endpoint, started, StatusCode::NOT_FOUND, "unknown_provider", &msg, &body).await;
        }
        tracing::debug!("provider override '{override_name}' reduced targets {before} -> {}", targets.len());
    }

    // 4) Streaming?
    let wants_stream = req_json
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let is_messages_endpoint = endpoint == "/v1/messages";
    let native_anthropic = is_messages_endpoint;

    // Live-Event: request gestartet (provider = erstes target; bei fallback
    // wechselt der effektive provider, sichtbar im completed-event)
    let primary_target = targets.first();
    state.log_sink.emit(LiveEvent::RequestStarted {
        request_id: request_id.clone(),
        timestamp: chrono::Utc::now(),
        key_name: vk.name.clone(),
        provider: primary_target
            .map(|t| t.provider_kind.as_str().to_string())
            .unwrap_or_default(),
        provider_name: primary_target
            .map(|t| t.provider_name.clone())
            .unwrap_or_default(),
        model: requested_model.clone(),
        endpoint: endpoint.clone(),
        is_stream: wants_stream,
    });

    // 5) Request gegen Targets ausfuehren (retry + fallback)
    let mut last_err: Option<ProviderError> = None;
    for (attempt, target) in targets.iter().enumerate() {
        let attempt_start = Instant::now();
        match execute_once(&state, target, &vk, &endpoint, &req_json, &request_id, wants_stream, native_anthropic).await {
            Ok(outcome) => {
                // streaming wird separat geloggt (log_stream_completion), sobald der
                // stream fertig ist - hier nicht doppelt loggen
                if outcome.log.is_stream && outcome.log.status == 0 {
                    return outcome.response;
                }
                // success: loggen und response zurueckgeben
                let duration_ms = started.elapsed().as_millis() as u64;
                let log = build_log(
                    &state,
                    Some(&vk),
                    &request_id,
                    &endpoint,
                    &req_json,
                    &target,
                    &outcome,
                    duration_ms,
                    attempt_start.elapsed().as_millis() as u64,
                    attempt,
                );
                let cost = log.cost_usd;
                state.log_sink.log(log);
                if cost > 0.0 {
                    state.track_spend(vk.id, cost);
                }
                return outcome.response;
            }
            Err(err) => {
                tracing::warn!(
                    "provider attempt {}/{} for model '{}' failed: {err}",
                    attempt + 1,
                    targets.len(),
                    requested_model,
                );
                if !err.is_retryable() {
                    last_err = Some(err);
                    break;
                }
                last_err = Some(err);
                if attempt < targets.len() - 1 && attempt < MAX_RETRIES + targets.len() {
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        }
    }

    // 6) alle Versuche fehlgeschlagen
    let err = last_err.unwrap_or(ProviderError::Status { status: 502, body: "no provider available".into() });
    let status = StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::BAD_GATEWAY);
    let error_type = if matches!(err, ProviderError::Network(_)) { "provider_network" } else { "provider_error" };
    error_response(state, Some(&vk), &request_id, &endpoint, started, status, error_type, &err.to_string(), &body).await
}

/// Ein einzelner Provider-Versuch.
#[allow(clippy::too_many_arguments)]
async fn execute_once(
    state: &AppState,
    target: &RouteTarget,
    vk: &common::state::VirtualKey,
    endpoint: &str,
    req_json: &Value,
    request_id: &str,
    wants_stream: bool,
    native_anthropic: bool,
) -> Result<ProxyOutcome, ProviderError> {
    match target.provider_kind {
        ProviderKind::OpenAi | ProviderKind::OpenAiCompat => {
            openai_call(state, target, vk, endpoint, req_json, request_id, wants_stream).await
        }
        ProviderKind::Anthropic => {
            anthropic_call(state, target, vk, endpoint, req_json, request_id, wants_stream, native_anthropic).await
        }
        ProviderKind::Gemini => {
            gemini_call(state, target, vk, endpoint, req_json, request_id, wants_stream).await
        }
    }
}

// ============================================================
// Provider-Aufrufe
// ============================================================

async fn openai_call(
    state: &AppState,
    target: &RouteTarget,
    vk: &common::state::VirtualKey,
    endpoint: &str,
    req_json: &Value,
    request_id: &str,
    wants_stream: bool,
) -> Result<ProxyOutcome, ProviderError> {
    let mut body = req_json.clone();
    if wants_stream {
        body["stream"] = json!(true);
        // stream_options fuer usage in streaming
        body["stream_options"] = json!({ "include_usage": true });
    } else {
        body.as_object_mut().map(|o| o.remove("stream"));
    }

    let mut upstream_body = body.clone();
    upstream_body["model"] = json!(target.upstream_model);

    let url = providers::openai::OpenAiAdapter::endpoint_url(&target.base_url, endpoint);
    let resp = state
        .http
        .post(&url)
        .bearer_auth(&target.api_key)
        .json(&upstream_body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Status { status: status.as_u16(), body: text });
    }

    if wants_stream && endpoint == "/v1/chat/completions" {
        // sse-durchreichen mit model-name rewrite
        let model_name = target.model_name.clone();
        Ok(stream_sse_passthrough(state, target, vk, endpoint, req_json, request_id, resp, move |chunk| {
            rewrite_stream_chunk_model(&chunk, &model_name).map(bytes::Bytes::from)
        }, StreamFormat::OpenAi)
        .await)
    } else {
        let bytes = resp.bytes().await?;
        let resp_json: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Status { status: 502, body: format!("invalid upstream json: {e}") })?;

        // model-name im response auf gateway-namen zuruecksetzen
        let mut out = resp_json;
        if !target.model_name.is_empty() {
            out["model"] = json!(target.model_name);
        }

        let bytes = serde_json::to_vec(&out).unwrap();
        let usage = providers::openai::OpenAiAdapter::extract_usage(&out);
        Ok(ProxyOutcome {
            response: json_response(StatusCode::OK, bytes::Bytes::from(bytes.clone())),
            log: build_log_from_response(
                state, target, endpoint, req_json, StatusCode::OK,
                &out, usage.0, usage.1, false, "",
            ),
        })
    }
}

async fn anthropic_call(
    state: &AppState,
    target: &RouteTarget,
    vk: &common::state::VirtualKey,
    endpoint: &str,
    req_json: &Value,
    request_id: &str,
    wants_stream: bool,
    native_anthropic: bool,
) -> Result<ProxyOutcome, ProviderError> {
    // 1) Request-Body im Zielformat bauen
    let (upstream_body, is_native) = if native_anthropic {
        (req_json.clone(), true)
    } else {
        (providers::translate::openai_request_to_anthropic(req_json, &target.upstream_model), false)
    };
    let mut upstream_body = upstream_body;
    upstream_body["model"] = json!(target.upstream_model);
    if wants_stream {
        upstream_body["stream"] = json!(true);
    }

    // 2) Request ausfuehren
    let resp = state
        .http
        .post(format!("{}/v1/messages", target.base_url.trim_end_matches('/')))
        .header("x-api-key", &target.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&upstream_body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Status { status: status.as_u16(), body: text });
    }

    if wants_stream {
        if is_native {
            // nativer anthropic-stream: 1:1 durchreichen
            Ok(stream_sse_passthrough(state, target, vk, endpoint, req_json, request_id, resp, |chunk| Some(chunk), StreamFormat::Anthropic).await)
        } else {
            // anthropic-stream -> openai-chunks uebersetzen
            Ok(stream_anthropic_to_openai(state, target, vk, endpoint, req_json, request_id, resp).await)
        }
    } else {
        let bytes = resp.bytes().await?;
        let anthropic_resp: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Status { status: 502, body: format!("invalid upstream json: {e}") })?;

        let usage = providers::anthropic::AnthropicAdapter::extract_usage(&anthropic_resp);
        let (out, log_resp_json) = if is_native {
            let mut o = anthropic_resp.clone();
            o["model"] = json!(target.model_name);
            (o.clone(), o)
        } else {
            let o = providers::translate::anthropic_response_to_openai(
                &anthropic_resp,
                &target.model_name,
                chrono::Utc::now().timestamp(),
            );
            (o.clone(), o)
        };

        let bytes = serde_json::to_vec(&out).unwrap();
        Ok(ProxyOutcome {
            response: json_response(StatusCode::OK, bytes::Bytes::from(bytes)),
            log: build_log_from_response(
                state, target, endpoint, req_json, StatusCode::OK,
                &log_resp_json, usage.0, usage.1, false, "",
            ),
        })
    }
}

async fn gemini_call(
    state: &AppState,
    target: &RouteTarget,
    vk: &common::state::VirtualKey,
    endpoint: &str,
    req_json: &Value,
    request_id: &str,
    wants_stream: bool,
) -> Result<ProxyOutcome, ProviderError> {
    if endpoint != "/v1/chat/completions" {
        return Err(ProviderError::Status {
            status: 400,
            body: format!("gemini provider does not support endpoint {endpoint}"),
        });
    }

    let gemini_body = providers::translate::openai_request_to_gemini(req_json);
    let method = if wants_stream { "streamGenerateContent" } else { "generateContent" };
    let url = format!(
        "{}/models/{}:{}?key={}&alt=sse",
        target.base_url.trim_end_matches('/'),
        target.upstream_model,
        method,
        target.api_key
    );

    let resp = state.http.post(&url).json(&gemini_body).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Status { status: status.as_u16(), body: text });
    }

    if wants_stream {
        // gemini SSE -> openai chunks
        Ok(stream_gemini_to_openai(state, target, vk, endpoint, req_json, request_id, resp).await)
    } else {
        let bytes = resp.bytes().await?;
        let gemini_resp: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ProviderError::Status { status: 502, body: format!("invalid upstream json: {e}") })?;

        let usage = (
            gemini_resp.pointer("/usageMetadata/promptTokenCount").and_then(|v| v.as_u64()).unwrap_or(0),
            gemini_resp.pointer("/usageMetadata/candidatesTokenCount").and_then(|v| v.as_u64()).unwrap_or(0),
        );
        let out = providers::translate::gemini_response_to_openai(
            &gemini_resp,
            &target.model_name,
            chrono::Utc::now().timestamp(),
        );

        let bytes = serde_json::to_vec(&out).unwrap();
        Ok(ProxyOutcome {
            response: json_response(StatusCode::OK, bytes::Bytes::from(bytes)),
            log: build_log_from_response(
                state, target, endpoint, req_json, StatusCode::OK,
                &out, usage.0, usage.1, false, "",
            ),
        })
    }
}

// ============================================================
// Streaming
// ============================================================

/// Gemeinsamer fehlerzustand fuer einen stream. Wird vom streaming-task
/// gesetzt, von log_stream_completion gelesen nachdem der log-channel
/// geschlossen ist, damit jeder am stream-ende gesetzte fehler sichtbar ist.
///
/// Client-disconnects werden bewusst als erfolg geloggt; die unterscheidung
/// zu einem sauberen stream-ende wuerde einen drop-guard brauchen.
#[derive(Default)]
struct StreamHealth {
    transport_error: std::sync::Mutex<Option<String>>,
    in_band_error: std::sync::Mutex<Option<String>>,
}

impl StreamHealth {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Upstream hat die verbindung mitten im stream abgebrochen (transport-fehler).
    fn set_transport_error(&self, message: String) {
        *self.transport_error.lock().unwrap() = Some(message);
    }

    /// Provider hat ein fehler-event im stream gesendet (das erste gewinnt).
    fn set_in_band_error(&self, message: String) {
        let mut g = self.in_band_error.lock().unwrap();
        if g.is_none() {
            *g = Some(message);
        }
    }

    fn transport_error(&self) -> Option<String> {
        self.transport_error.lock().unwrap().clone()
    }

    fn in_band_error(&self) -> Option<String> {
        self.in_band_error.lock().unwrap().clone()
    }
}

/// Drained den log-channel bis zum channel-schluss (damit health-fehler am
/// stream-ende nicht verloren gehen) und behaelt nur die neusten 256 KB
/// (tail), damit spaete in-band-fehler-events und der finale usage-chunk im
/// transcript bleiben.
async fn drain_log_rx(mut log_rx: tokio::sync::mpsc::Receiver<bytes::Bytes>) -> Vec<u8> {
    let limit = 256 * 1024;
    let mut chunks: Vec<bytes::Bytes> = Vec::new();
    let mut total: usize = 0;
    while let Some(part) = log_rx.recv().await {
        chunks.push(part);
        total += chunks.last().unwrap().len();
        // bei ueberschreitung von vorn verwerfen, damit der stream-tail bleibt
        while total > limit {
            let front_len = chunks[0].len();
            if front_len > total - limit {
                let drop = total - limit;
                let front = chunks.remove(0);
                total -= drop;
                chunks.insert(0, front.slice(drop..));
                break;
            }
            total -= front_len;
            chunks.remove(0);
        }
    }
    let mut full = Vec::with_capacity(total);
    for c in &chunks {
        full.extend_from_slice(c);
    }
    full
}

/// SSE-Bytes 1:1 an den Client durchreichen (mit optionalem rewrite pro chunk).
#[allow(clippy::too_many_arguments)]
async fn stream_sse_passthrough<F>(
    state: &AppState,
    target: &RouteTarget,
    vk: &common::state::VirtualKey,
    endpoint: &str,
    req_json: &Value,
    request_id: &str,
    resp: reqwest::Response,
    rewrite: F,
    log_format: StreamFormat,
) -> ProxyOutcome
where
    F: Fn(bytes::Bytes) -> Option<bytes::Bytes> + Send + Sync + 'static,
{
    let started = Instant::now();
    let first_byte = Arc::new(std::sync::Mutex::new(Option::<u64>::None));
    let fb_clone = first_byte.clone();
    let start_clone = started;
    let live_sink = state.log_sink.clone();
    let rid_clone = request_id.to_string();

    // transport-fehler (upstream bricht mitten im stream ab) gehen nur in
    // health: der client-stream bleibt sauber (leerer chunk), nur der
    // log-status wird 502.
    let health = StreamHealth::new();
    let health_map = health.clone();
    let stream = resp
        .bytes_stream()
        .map(move |chunk| {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    health_map.set_transport_error(e.to_string());
                    bytes::Bytes::new()
                }
            };
            // first byte nur bei erstem nicht-leerem chunk (ein leerer
            // chunk aus dem err-zweig darf nicht feuern)
            let mut fb_ms = None;
            if !chunk.is_empty() {
                let mut guard = fb_clone.lock().unwrap();
                if guard.is_none() {
                    fb_ms = Some(start_clone.elapsed().as_millis() as u64);
                    *guard = fb_ms;
                }
            }
            if let Some(ms) = fb_ms {
                live_sink.emit(LiveEvent::FirstByte {
                    request_id: rid_clone.clone(),
                    first_byte_ms: ms,
                });
            }
            Ok::<_, std::io::Error>(chunk)
        });

    // sammelt response-fragmente parallel im hintergrund fuer das log
    let (log_tx, log_rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(1000);
    let intercepted = stream.map(move |chunk: Result<bytes::Bytes, std::io::Error>| {
        if let Ok(data) = &chunk {
            let _ = log_tx.try_send(data.clone());
        }
        chunk.map(|c| rewrite(c).unwrap_or_default())
    });

    // log-collector: channel wird bis zum ende drained (tail-transcript)
    let log_collector = tokio::spawn(async move { drain_log_rx(log_rx).await });

    let body = Body::from_stream(intercepted);

    // log-task: wenn der stream beendet ist (collector fertig), log bauen
    log_stream_completion(
        state.clone(),
        target.clone(),
        vk.clone(),
        request_id.to_string(),
        endpoint.to_string(),
        req_json.clone(),
        started,
        first_byte,
        log_collector,
        health,
        log_format,
        true,
    );

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", axum::http::HeaderValue::from_static("text/event-stream"));
    headers.insert("cache-control", axum::http::HeaderValue::from_static("no-cache"));

    ProxyOutcome {
        response: Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .unwrap(),
        log: empty_log(state, target, endpoint, req_json),
    }
}

/// Anthropic-SSE-stream in OpenAI-chunk-format uebersetzen.
async fn stream_anthropic_to_openai(
    state: &AppState,
    target: &RouteTarget,
    vk: &common::state::VirtualKey,
    endpoint: &str,
    req_json: &Value,
    request_id: &str,
    resp: reqwest::Response,
) -> ProxyOutcome {
    let started = Instant::now();
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(100);
    let (log_tx, log_rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(100);
    let health = StreamHealth::new();
    let health_task = health.clone();

    let target_clone = target.clone();
    let model_name = target.model_name.clone();
    tokio::spawn(async move {
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut message_id = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    health_task.set_transport_error(e.to_string());
                    let _ = out_tx.send(Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))).await;
                    break;
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // komplette SSE-events verarbeiten
            while let Some(pos) = buffer.find("\n\n") {
                let raw_event = buffer[..pos].to_string();
                buffer.drain(..pos + 2);

                let mut event_name = "";
                let mut data_str = "";
                for line in raw_event.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event_name = v.trim();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data_str = v.trim();
                    }
                }
                if data_str.is_empty() {
                    continue;
                }
                let data: Value = match serde_json::from_str(data_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // in-band fehler-event: der konverter uebersetzt es nicht
                // (liefert keinen chunk zurueck), es kommt nie ins
                // log-transcript. daher hier in health aufnehmen, statt auf
                // transcript-scan zu vertrauen.
                if event_name == "error"
                    || data.get("type").and_then(|t| t.as_str()) == Some("error")
                {
                    health_task.set_in_band_error(error_event_message(&data));
                    continue;
                }

                // message-id merken
                if event_name == "message_start" {
                    if let Some(id) = data.pointer("/message/id").and_then(|v| v.as_str()) {
                        message_id = id.to_string();
                    }
                }

                let mut data = data;
                if !message_id.is_empty() {
                    data["_message_id"] = json!(message_id);
                }

                if let Some(openai_chunk) = providers::translate::anthropic_stream_event_to_openai(
                    event_name, &data, &model_name,
                ) {
                    // auch die uebersetzten chunks fuer das log mitschneiden
                    let _ = log_tx.send(bytes::Bytes::from(format_sse(&openai_chunk))).await;
                    let payload = format_sse(&openai_chunk);
                    if out_tx.send(Ok(payload.into())).await.is_err() {
                        return;
                    }
                }
            }
        }
        // [DONE] senden (openai-konvention)
        let _ = out_tx.send(Ok(bytes::Bytes::from("data: [DONE]\n\n"))).await;
        let _ = target_clone;
    });

    let first_byte = Arc::new(std::sync::Mutex::new(None::<u64>));
    let fb_clone = first_byte.clone();
    let start_clone = started;
    let live_sink = state.log_sink.clone();
    let rid_clone = request_id.to_string();
    let stream = tokio_stream::wrappers::ReceiverStream::new(out_rx).map(move |chunk| {
        // first byte nur bei erstem nicht-leerem ok-chunk (err-items und
        // leere chunks duerfen nicht feuern)
        let mut fb_ms = None;
        if let Ok(c) = &chunk {
            if !c.is_empty() {
                let mut guard = fb_clone.lock().unwrap();
                if guard.is_none() {
                    fb_ms = Some(start_clone.elapsed().as_millis() as u64);
                    *guard = fb_ms;
                }
            }
        }
        if let Some(ms) = fb_ms {
            live_sink.emit(LiveEvent::FirstByte {
                request_id: rid_clone.clone(),
                first_byte_ms: ms,
            });
        }
        chunk
    });

    let log_collector = tokio::spawn(async move { drain_log_rx(log_rx).await });

    log_stream_completion(
        state.clone(),
        target.clone(),
        vk.clone(),
        request_id.to_string(),
        endpoint.to_string(),
        req_json.clone(),
        started,
        first_byte,
        log_collector,
        health,
        StreamFormat::OpenAi,
        false,
    );

    let body = Body::from_stream(stream);
    ProxyOutcome {
        response: Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .unwrap(),
        log: empty_log(state, target, endpoint, req_json),
    }
}

/// Gemini-SSE-stream in OpenAI-chunk-format uebersetzen.
async fn stream_gemini_to_openai(
    state: &AppState,
    target: &RouteTarget,
    vk: &common::state::VirtualKey,
    endpoint: &str,
    req_json: &Value,
    request_id: &str,
    resp: reqwest::Response,
) -> ProxyOutcome {
    let started = Instant::now();
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(100);
    let (log_tx, log_rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(100);
    let health = StreamHealth::new();
    let health_task = health.clone();

    let model_name = target.model_name.clone();
    tokio::spawn(async move {
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    health_task.set_transport_error(e.to_string());
                    let _ = out_tx.send(Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))).await;
                    break;
                }
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find("\n\n") {
                let raw_event = buffer[..pos].to_string();
                buffer.drain(..pos + 2);

                let data_str = raw_event
                    .lines()
                    .find_map(|l| l.strip_prefix("data:"))
                    .map(|s| s.trim())
                    .unwrap_or("");
                if data_str.is_empty() {
                    continue;
                }
                let data: Value = match serde_json::from_str(data_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                // in-band fehler-objekt: der konverter ignoriert es (keine
                // candidates), daher hier in health aufnehmen.
                if gemini_in_band_error(&data) {
                    health_task.set_in_band_error(error_event_message(&data));
                    continue;
                }
                for openai_chunk in providers::translate::gemini_chunk_to_openai(&data, &model_name) {
                    // auch die uebersetzten chunks fuer das log mitschneiden
                    let _ = log_tx.send(bytes::Bytes::from(format_sse(&openai_chunk))).await;
                    let payload = format_sse(&openai_chunk);
                    if out_tx.send(Ok(payload.into())).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = out_tx.send(Ok(bytes::Bytes::from("data: [DONE]\n\n"))).await;
    });

    let first_byte = Arc::new(std::sync::Mutex::new(None::<u64>));
    let fb_clone = first_byte.clone();
    let start_clone = started;
    let live_sink = state.log_sink.clone();
    let rid_clone = request_id.to_string();
    let stream = tokio_stream::wrappers::ReceiverStream::new(out_rx).map(move |chunk| {
        // first byte nur bei erstem nicht-leerem ok-chunk (err-items und
        // leere chunks duerfen nicht feuern)
        let mut fb_ms = None;
        if let Ok(c) = &chunk {
            if !c.is_empty() {
                let mut guard = fb_clone.lock().unwrap();
                if guard.is_none() {
                    fb_ms = Some(start_clone.elapsed().as_millis() as u64);
                    *guard = fb_ms;
                }
            }
        }
        if let Some(ms) = fb_ms {
            live_sink.emit(LiveEvent::FirstByte {
                request_id: rid_clone.clone(),
                first_byte_ms: ms,
            });
        }
        chunk
    });

    let log_collector = tokio::spawn(async move { drain_log_rx(log_rx).await });

    log_stream_completion(
        state.clone(),
        target.clone(),
        vk.clone(),
        request_id.to_string(),
        endpoint.to_string(),
        req_json.clone(),
        started,
        first_byte,
        log_collector,
        health,
        StreamFormat::OpenAi,
        false,
    );

    let body = Body::from_stream(stream);
    ProxyOutcome {
        response: Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(body)
            .unwrap(),
        log: empty_log(state, target, endpoint, req_json),
    }
}

/// Nach Abschluss eines Streams: log-entry bauen und einspeisen.
/// Der roh-SSE-mitschnitt wird zu einem kompakten chat.completion-JSON
/// verdichtet (text, usage, finish_reason) statt 1:1 geloggt.
#[allow(clippy::too_many_arguments)]
fn log_stream_completion(
    state: AppState,
    target: RouteTarget,
    vk: common::state::VirtualKey,
    request_id: String,
    endpoint: String,
    req_json: Value,
    started: Instant,
    first_byte: Arc<std::sync::Mutex<Option<u64>>>,
    log_collector: tokio::task::JoinHandle<Vec<u8>>,
    health: Arc<StreamHealth>,
    format: StreamFormat,
    // transcript auf in-band fehler-events scannen: nur im passthrough, wo
    // fehler-events untransformiert in den log-channel landen. in den
    // konvertierten pfaden geht das durch health (scan waere toter code).
    scan_transcript: bool,
) {
    tokio::spawn(async move {
        let response_bytes = log_collector.await.unwrap_or_default();
        let duration_ms = started.elapsed().as_millis() as u64;
        let first_byte_ms = first_byte.lock().unwrap().unwrap_or(0);

        // EINE UUID fuer Live-Event UND persistiertes Log, damit das
        // Dashboard die Live-Row auf /requests/:id verlinken kann.
        let log_id = Uuid::new_v4();

        // EINE Zeitstempel-Quelle fuer Live-Event UND persistiertes Log, damit
        // beide zeilen exakt identische timestamps tragen.
        let timestamp = chrono::Utc::now();

        let response_str = String::from_utf8_lossy(&response_bytes).to_string();

        // usage aus dem stream extrahieren (status/error kommen ausschliesslich
        // aus apply_stream_health; ohne fehler ist der stream ein erfolg)
        let (prompt_tokens, completion_tokens) = extract_stream_usage(&response_str, &target);
        let (mut status, mut error_message, mut error_type) = (200, String::new(), String::new());
        // stream-fehler (transport oder in-band) ueberschreiben den status;
        // client-disconnects bleiben 200 (siehe StreamHealth).
        if let Some((s, et, em)) = apply_stream_health(&health, &response_str, format, scan_transcript) {
            status = s;
            error_type = et;
            error_message = em;
        }

        let estimated_prompt = if prompt_tokens > 0 {
            prompt_tokens
        } else {
            estimate_tokens(&req_json.to_string())
        };
        let estimated_completion = if completion_tokens > 0 {
            completion_tokens
        } else {
            estimate_tokens(&extract_stream_text(&response_str))
        };

        let cost = compute_cost(
            estimated_prompt,
            estimated_completion,
            target.input_price_per_million,
            target.output_price_per_million,
        );

        // in-flight-zähler sofort zuruecksetzen, bevor die teure log-aufbereitung
        // (stream_to_completion_json) laeuft. Das live-dashboard sieht damit den
        // ende-zustand ohne verzögerung. Werte sind identisch zur persistierten
        // Zeile (gleiche UUID, tokens, cost).
        state.log_sink.emit(LiveEvent::Completed {
            log: LiveLog {
                id: log_id,
                request_id: request_id.clone(),
                timestamp,
                key_name: vk.name.clone(),
                provider: target.provider_kind.as_str().to_string(),
                provider_name: target.provider_name.clone(),
                model: target.model_name.clone(),
                upstream_model: target.upstream_model.clone(),
                endpoint: endpoint.clone(),
                status,
                error_type: error_type.clone(),
                is_stream: true,
                prompt_tokens: estimated_prompt,
                completion_tokens: estimated_completion,
                cost_usd: cost,
                duration_ms,
                first_byte_ms,
            },
        });

        // kompaktes completion-json statt roh-SSE loggen (teuer, deshalb
        // erst nach dem Emit)
        let response_body = if response_str.is_empty() {
            String::new()
        } else {
            stream_to_completion_json(&response_str, format, &target.model_name, &request_id)
        };

        let log = RequestLog {
            id: log_id,
            request_id,
            timestamp,
            virtual_key_id: Some(vk.id),
            key_name: vk.name.clone(),
            provider: target.provider_kind.as_str().to_string(),
            provider_name: target.provider_name.clone(),
            provider_id: Some(target.provider_id),
            model: target.model_name.clone(),
            upstream_model: target.upstream_model.clone(),
            endpoint,
            status,
            error_message,
            error_type,
            is_stream: true,
            prompt_tokens: estimated_prompt,
            completion_tokens: estimated_completion,
            total_tokens: estimated_prompt + estimated_completion,
            cost_usd: cost,
            duration_ms,
            first_byte_ms,
            request_body: req_json.to_string(),
            response_body,
            request_truncated: false,
            response_truncated: false,
        };
        if log.cost_usd > 0.0 {
            state.track_spend(vk.id, log.cost_usd);
        }
        state.log_sink.log_only(log);
    });
}

/// Erkennt ein in-band fehler-event des providers im SSE-transcript.
/// Nur ein top-level "error"-key zaehlt; "error" in einem content-string
/// ist kein fehler.
fn detect_in_band_error(transcript: &str, format: StreamFormat) -> Option<String> {
    match format {
        StreamFormat::OpenAi => {
            for line in transcript.lines() {
                if !line.contains("\"error\"") {
                    continue;
                }
                let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                if v.get("error").map(|e| !e.is_null()).unwrap_or(false) {
                    return Some(error_event_message(&v["error"]));
                }
            }
            None
        }
        StreamFormat::Anthropic => {
            let mut in_error_event = false;
            for line in transcript.lines() {
                if let Some(name) = line.strip_prefix("event:") {
                    in_error_event = name.trim() == "error";
                    continue;
                }
                let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                let is_error = in_error_event
                    || v.get("type").and_then(|t| t.as_str()) == Some("error");
                in_error_event = false;
                if is_error {
                    return Some(error_event_message(&v));
                }
            }
            None
        }
    }
}

/// Kurze fehlermeldung aus einem provider-fehler-objekt. Akzeptiert das
/// "error"-objekt selbst oder das ganze event, das es enthaelt.
fn error_event_message(data: &Value) -> String {
    let err = if data.get("error").is_some() { &data["error"] } else { data };
    let msg = err
        .get("message")
        .and_then(|m| m.as_str())
        .or_else(|| err.get("type").and_then(|t| t.as_str()));
    msg.map(|s| format!("upstream stream error: {s}"))
        .unwrap_or_else(|| "upstream stream error event".to_string())
}

/// Mappt stream-health auf einen log-status-override: transport-fehler
/// gewinnen gegen in-band-fehler bei error_type, aber beide messages werden
/// kombiniert (die provider-begruendung geht nicht verloren). None = kein
/// override (status bleibt 200).
///
/// In-Band-Fehler-Events loggen 502/provider_error. Das ist eine bewusste
/// abweichung vom non-streaming-pfad (dort landet der echte upstream-status
/// im log), weil sse-error-events keinen http-status tragen.
fn apply_stream_health(
    health: &StreamHealth,
    transcript: &str,
    format: StreamFormat,
    scan_transcript: bool,
) -> Option<(u16, String, String)> {
    let in_band = health.in_band_error().or_else(|| {
        scan_transcript.then(|| detect_in_band_error(transcript, format)).flatten()
    });
    match (health.transport_error(), in_band) {
        (Some(t), Some(i)) => Some((
            502,
            "provider_network".to_string(),
            format!("{t}; in-band: {i}"),
        )),
        (Some(t), None) => Some((502, "provider_network".to_string(), t)),
        (None, Some(i)) => Some((502, "provider_error".to_string(), i)),
        (None, None) => None,
    }
}

/// Gemini-in-band-fehler: top-level "error"-objekt, das nicht null ist.
fn gemini_in_band_error(value: &Value) -> bool {
    value.get("error").map(|e| !e.is_null()).unwrap_or(false)
}

/// Versucht usage aus einem SSE-response-mitschnitt zu extrahieren.
/// Status/error gibt es hier nicht — die kommen aus apply_stream_health.
fn extract_stream_usage(response: &str, target: &RouteTarget) -> (u64, u64) {
    match target.provider_kind {
        ProviderKind::OpenAi | ProviderKind::OpenAiCompat => {
            // letzter chunk mit usage
            for line in response.lines().rev() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" { continue; }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                            let p = u.pointer("/prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                            let c = u.pointer("/completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                            if p > 0 || c > 0 {
                                return (p, c);
                            }
                        }
                    }
                }
            }
            (0, 0)
        }
        ProviderKind::Anthropic => {
            for line in response.lines().rev() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(u) = v.get("usage") {
                            let p = u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                            let c = u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                            if p > 0 || c > 0 {
                                return (p, c);
                            }
                        }
                    }
                }
            }
            (0, 0)
        }
        ProviderKind::Gemini => {
            for line in response.lines().rev() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        if let Some(u) = v.get("usageMetadata") {
                            let p = u.get("promptTokenCount").and_then(|x| x.as_u64()).unwrap_or(0);
                            let c = u.get("candidatesTokenCount").and_then(|x| x.as_u64()).unwrap_or(0);
                            if p > 0 || c > 0 {
                                return (p, c);
                            }
                        }
                    }
                }
            }
            (0, 0)
        }
    }
}

/// Extrahiert sichtbaren text aus einem openai-SSE-mitschnitt (fuer token-schaetzung).
fn extract_stream_text(response: &str) -> String {
    let mut text = String::new();
    for line in response.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" { continue; }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(deltas) = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
                    text.push_str(deltas);
                }
            }
        }
    }
    text
}

// ============================================================
// Stream-Logging: synthetisches chat.completion-JSON
// ============================================================

/// Format des SSE-mitschnitts, entspricht dem was in log_collector landet.
#[derive(Debug, Clone, Copy, PartialEq)]
enum StreamFormat {
    /// OpenAI-chunk-format (openai-passthrough, anthropic/gemini uebersetzt)
    OpenAi,
    /// Anthropic-rohevents (native /v1/messages streams)
    Anthropic,
}

/// Baut aus einem SSE-mitschnitt ein kompaktes chat.completion-JSON:
/// gesammelter assistant-text, usage, finish_reason. 10-20x kleiner als
/// der roh-mitschnitt und identisch formatiert wie non-streaming logs.
fn stream_to_completion_json(response: &str, format: StreamFormat, model_name: &str, request_id: &str) -> String {
    let (text, reasoning, finish_reason, usage, saw_done, tool_calls) = match format {
        StreamFormat::OpenAi => {
            let (text, reasoning, finish, usage, saw_done, tool_calls) = openai_stream_parts(response);
            (text, reasoning, finish, usage, saw_done, tool_calls)
        }
        StreamFormat::Anthropic => {
            let (text, finish, usage, saw_done, tool_calls) = anthropic_stream_parts(response);
            (text, String::new(), finish, usage, saw_done, tool_calls)
        }
    };
    let (prompt_tokens, completion_tokens) = usage;

    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { json!(text) },
    });
    if !reasoning.is_empty() {
        message["reasoning"] = json!(reasoning);
    }

    let mut completion = json!({
        "id": format!("chatcmpl-{request_id}"),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model_name,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": if finish_reason.is_empty() { Value::Null } else { json!(finish_reason) },
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        },
    });
    if !tool_calls.is_empty() {
        completion["choices"][0]["message"]["tool_calls"] = json!(tool_calls);
    }
    // mitschnitt unabgeschlossen (collector-limit erreicht oder stream abgebrochen)
    if !saw_done {
        completion["_truncated"] = json!(true);
    }
    serde_json::to_string_pretty(&completion).unwrap_or_default()
}

/// Extrahiert (text, finish_reason, (prompt, completion), saw_done, tool_calls)
/// aus einem openai-chunk-SSE-mitschnitt.
fn openai_stream_parts(response: &str) -> (String, String, String, (u64, u64), bool, Vec<Value>) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut finish_reason = String::new();
    let mut usage = (0u64, 0u64);
    let mut saw_done = false;
    // tool_calls nach index sammeln: (index, id, name, arguments-buffer)
    let mut tool_calls: Vec<(u64, String, String, String)> = Vec::new();

    for line in response.lines() {
        let Some(data) = line.strip_prefix("data: ") else { continue };
        if data == "[DONE]" {
            saw_done = true;
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };

        if let Some(delta_text) = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
            text.push_str(delta_text);
        }
        // reasoning-deltas (z.b. qwen3 via vllm): separat sammeln
        if let Some(delta_text) = v.pointer("/choices/0/delta/reasoning").and_then(|c| c.as_str()) {
            reasoning.push_str(delta_text);
        }
        if let Some(fr) = v.pointer("/choices/0/finish_reason").and_then(|f| f.as_str()) {
            finish_reason = fr.to_string();
        }
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            let p = u.pointer("/prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            let c = u.pointer("/completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            if p > 0 || c > 0 {
                usage = (p, c);
            }
        }
        // tool_call-deltas: argument-fragmente konkatenieren
        if let Some(calls) = v.pointer("/choices/0/delta/tool_calls").and_then(|t| t.as_array()) {
            for call in calls {
                let idx = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                let entry = tool_calls.iter_mut().find(|(i, _, _, _)| *i == idx);
                let entry = match entry {
                    Some(e) => e,
                    None => {
                        tool_calls.push((idx, String::new(), String::new(), String::new()));
                        tool_calls.last_mut().unwrap()
                    }
                };
                if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                    if !id.is_empty() { entry.1 = id.to_string(); }
                }
                if let Some(name) = call.pointer("/function/name").and_then(|n| n.as_str()) {
                    if !name.is_empty() { entry.2 = name.to_string(); }
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(|a| a.as_str()) {
                    entry.3.push_str(args);
                }
            }
        }
    }

    let tool_calls: Vec<Value> = tool_calls
        .into_iter()
        .map(|(index, id, name, arguments)| {
            json!({
                "index": index,
                "id": id,
                "type": "function",
                "function": { "name": name, "arguments": arguments },
            })
        })
        .collect();

    (text, reasoning, finish_reason, usage, saw_done, tool_calls)
}

/// Extrahiert die teile aus einem anthropic-rohstream (native /v1/messages):
/// content_block_delta/text_delta fuer text, message_delta fuer stop_reason/usage.
fn anthropic_stream_parts(response: &str) -> (String, String, (u64, u64), bool, Vec<Value>) {
    let mut text = String::new();
    let mut finish_reason = String::new();
    let mut usage = (0u64, 0u64);
    let mut saw_done = false;

    for line in response.lines() {
        let Some(data) = line.strip_prefix("data: ") else { continue };
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "content_block_delta" => {
                if let Some(delta_text) = v.pointer("/delta/text").and_then(|t| t.as_str()) {
                    text.push_str(delta_text);
                }
            }
            "message_delta" => {
                if let Some(stop) = v.pointer("/delta/stop_reason").and_then(|s| s.as_str()) {
                    finish_reason = stop.to_string();
                }
                if let Some(u) = v.get("usage") {
                    let p = u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                    let c = u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                    if p > 0 || c > 0 {
                        usage = (p, c);
                    }
                }
                saw_done = true; // message_delta ist das letzte relevante event vor message_stop
            }
            "message_stop" => {
                saw_done = true;
            }
            _ => {}
        }
    }

    // anthropic stop_reasons auf openai-namen mappen
    let finish_reason = match finish_reason.as_str() {
        "end_turn" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        "stop_sequence" => "stop".to_string(),
        other => other.to_string(),
    };

    (text, finish_reason, usage, saw_done, Vec::new())
}

fn rewrite_stream_chunk_model(chunk: &bytes::Bytes, model_name: &str) -> Option<String> {
    // ganze zeilenweise chunks: model-feld ersetzen (lightweight string-rewrite)
    let s = String::from_utf8_lossy(chunk).to_string();
    if !s.contains("\"model\"") {
        return Some(s);
    }
    let re = "\"model\":";
    if let Some(pos) = s.find(re) {
        // find the value bounds
        let after = &s[pos + re.len()..];
        let start = after.find('"')? + 1;
        let end = after[start..].find('"')? + start;
        let new = format!("{}\"{}\"{}", &s[..pos + re.len()], model_name, &after[end + 1..]);
        return Some(new);
    }
    Some(s)
}

// ============================================================
// Log-Building
// ============================================================

#[allow(clippy::too_many_arguments)]
fn build_log(
    _state: &AppState,
    vk: Option<&common::state::VirtualKey>,
    request_id: &str,
    endpoint: &str,
    req_json: &Value,
    target: &RouteTarget,
    outcome: &ProxyOutcome,
    duration_ms: u64,
    _upstream_ms: u64,
    attempt: usize,
) -> RequestLog {
    let (p, c) = extract_log_usage(&outcome.log);
    let cost = compute_cost(p, c, target.input_price_per_million, target.output_price_per_million);
    let is_stream = outcome.log.is_stream;
    let status = outcome.log.status;
    let request_body = req_json.to_string();
    let response_body = outcome.log.response_body.clone();
    let error_message = outcome.log.error_message.clone();
    let error_type = outcome.log.error_type.clone();
    let first_byte_ms = outcome.log.first_byte_ms;
    let _ = attempt;

    RequestLog {
        id: Uuid::new_v4(),
        request_id: request_id.to_string(),
        timestamp: chrono::Utc::now(),
        virtual_key_id: vk.map(|v| v.id),
        key_name: vk.map(|v| v.name.clone()).unwrap_or_default(),
        provider: target.provider_kind.as_str().to_string(),
        provider_name: target.provider_name.clone(),
        provider_id: Some(target.provider_id),
        model: target.model_name.clone(),
        upstream_model: target.upstream_model.clone(),
        endpoint: endpoint.to_string(),
        status,
        error_message,
        error_type,
        is_stream,
        prompt_tokens: p,
        completion_tokens: c,
        total_tokens: p + c,
        cost_usd: cost,
        duration_ms,
        first_byte_ms,
        request_body,
        response_body,
        request_truncated: false,
        response_truncated: false,
    }
}

fn extract_log_usage(log: &RequestLog) -> (u64, u64) {
    (log.prompt_tokens, log.completion_tokens)
}

#[allow(clippy::too_many_arguments)]
fn build_log_from_response(
    _state: &AppState,
    target: &RouteTarget,
    endpoint: &str,
    req_json: &Value,
    status: StatusCode,
    resp_json: &Value,
    prompt_tokens: u64,
    completion_tokens: u64,
    is_stream: bool,
    error_message: &str,
) -> RequestLog {
    let cost = compute_cost(
        prompt_tokens,
        completion_tokens,
        target.input_price_per_million,
        target.output_price_per_million,
    );
    RequestLog {
        id: Uuid::new_v4(),
        request_id: Uuid::new_v4().to_string(),
        timestamp: chrono::Utc::now(),
        virtual_key_id: None,
        key_name: String::new(),
        provider: target.provider_kind.as_str().to_string(),
        provider_name: target.provider_name.clone(),
        provider_id: Some(target.provider_id),
        model: target.model_name.clone(),
        upstream_model: target.upstream_model.clone(),
        endpoint: endpoint.to_string(),
        status: status.as_u16(),
        error_message: error_message.to_string(),
        error_type: if error_message.is_empty() { String::new() } else { "provider_error".into() },
        is_stream,
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
        cost_usd: cost,
        duration_ms: 0,
        first_byte_ms: 0,
        request_body: req_json.to_string(),
        response_body: serde_json::to_string(resp_json).unwrap_or_default(),
        request_truncated: false,
        response_truncated: false,
    }
}

fn empty_log(_state: &AppState, _target: &RouteTarget, _endpoint: &str, _req_json: &Value) -> RequestLog {
    RequestLog {
        id: Uuid::new_v4(),
        request_id: String::new(),
        timestamp: chrono::Utc::now(),
        virtual_key_id: None,
        key_name: String::new(),
        provider: String::new(),
        provider_name: String::new(),
        provider_id: None,
        model: String::new(),
        upstream_model: String::new(),
        endpoint: String::new(),
        status: 0,
        error_message: String::new(),
        error_type: String::new(),
        is_stream: true,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        cost_usd: 0.0,
        duration_ms: 0,
        first_byte_ms: 0,
        request_body: String::new(),
        response_body: String::new(),
        request_truncated: false,
        response_truncated: false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn error_response(
    state: AppState,
    vk: Option<&common::state::VirtualKey>,
    request_id: &str,
    endpoint: &str,
    started: Instant,
    status: StatusCode,
    error_type: &str,
    message: &str,
    req_body: &bytes::Bytes,
) -> Response {
    let duration_ms = started.elapsed().as_millis() as u64;
    let log = RequestLog {
        id: Uuid::new_v4(),
        request_id: request_id.to_string(),
        timestamp: chrono::Utc::now(),
        virtual_key_id: vk.map(|v| v.id),
        key_name: vk.map(|v| v.name.clone()).unwrap_or_default(),
        provider: String::new(),
        provider_name: String::new(),
        provider_id: None,
        model: String::new(),
        upstream_model: String::new(),
        endpoint: endpoint.to_string(),
        status: status.as_u16(),
        error_message: message.to_string(),
        error_type: error_type.to_string(),
        is_stream: false,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        cost_usd: 0.0,
        duration_ms,
        first_byte_ms: 0,
        request_body: String::from_utf8_lossy(req_body).to_string(),
        response_body: String::new(),
        request_truncated: false,
        response_truncated: false,
    };
    state.log_sink.log(log);

    let body = json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    });
    (status, axum::Json(body)).into_response()
}

fn status_text(status: &StatusCode) -> String {
    status.canonical_reason().unwrap_or("error").to_string()
}

fn format_sse(value: &Value) -> String {
    format!("data: {}\n\n", serde_json::to_string(value).unwrap_or_default())
}

fn json_response(status: StatusCode, body: bytes::Bytes) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

fn endpoint_from_uri(uri: &axum::http::Uri) -> String {
    uri.path().split('?').next().unwrap_or("/").to_string()
}

pub async fn models_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // auth wie proxy
    let vk = match auth::authenticate(&state, &headers).await {
        Ok(vk) => vk,
        Err(status) => {
            // 401 auf /v1/models laeuft nie in den Request-Logs (kein CH-Log);
            // explizit als Auth-Fehler zaehlen.
            if status == StatusCode::UNAUTHORIZED {
                state.log_sink.record_auth_failure();
            }
            return (status, axum::Json(json!({"error": status_text(&status)}))).into_response();
        }
    };
    let _ = vk;

    let routes = state.routes.read().await;
    let models: Vec<Value> = routes
        .iter()
        .map(|(name, targets)| {
            // capabilities des ersten (primären) targets; namensgeber des modells
            let caps = targets.first().and_then(|t| t.capabilities.clone());
            let mut m = json!({
                "id": name,
                "object": "model",
                "created": 0,
                "owned_by": "yalr",
            });
            if let Some(c) = caps {
                m["capabilities"] = c;
            }
            m
        })
        .collect();
    axum::Json(json!({ "object": "list", "data": models })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_stream_to_completion_json() {
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2,\"total_tokens\":9}}\n\n",
            "data: [DONE]\n\n",
        );
        let out = stream_to_completion_json(sse, StreamFormat::OpenAi, "gpt-4o", "req-1");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["model"], "gpt-4o");
        assert_eq!(v["choices"][0]["message"]["content"], "Hello world");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["prompt_tokens"], 7);
        assert_eq!(v["usage"]["completion_tokens"], 2);
        assert!(v.get("_truncated").is_none());
    }

    #[test]
    fn test_openai_stream_tool_calls() {
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Berlin\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (text, reasoning, finish, _usage, saw_done, tool_calls) = openai_stream_parts(sse);
        assert!(text.is_empty());
        assert!(reasoning.is_empty());
        assert_eq!(finish, "tool_calls");
        assert!(saw_done);
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_1");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
        assert_eq!(tool_calls[0]["function"]["arguments"], "{\"city\":\"Berlin\"}");
    }

    #[test]
    fn test_openai_stream_truncated_sets_flag() {
        // collector-limit erreicht: kein [DONE], kein finish_reason
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n";
        let out = stream_to_completion_json(sse, StreamFormat::OpenAi, "m", "r");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "partial");
        assert_eq!(v["_truncated"], true);
    }

    #[test]
    fn test_openai_stream_reasoning_collected() {
        // qwen3-style: reasoning-deltas statt/ neben content
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"denke\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\" laut\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Antwort\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let out = stream_to_completion_json(sse, StreamFormat::OpenAi, "qwen", "r1");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "Antwort");
        assert_eq!(v["choices"][0]["message"]["reasoning"], "denke laut");
    }

    #[test]
    fn test_anthropic_stream_to_completion_json() {
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let out = stream_to_completion_json(sse, StreamFormat::Anthropic, "claude", "req-2");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "Hi there");
        assert_eq!(v["choices"][0]["finish_reason"], "stop"); // end_turn -> stop
        assert_eq!(v["usage"]["prompt_tokens"], 5);
        assert_eq!(v["usage"]["completion_tokens"], 2);
        assert!(v.get("_truncated").is_none());
    }

    #[test]
    fn test_empty_stream_yields_empty_body() {
        // leerer mitschnitt (stream abgebrochen): log_stream_completion
        // laesst response_body leer; hier: extraction auf leerstring
        let (text, _reasoning, _finish, _usage, saw_done, _tools) = openai_stream_parts("");
        assert!(text.is_empty());
        assert!(!saw_done);
    }

    #[test]
    fn test_anthropic_stream_parts() {
        let sse = concat!(
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let (text, finish, usage, saw_done, tools) = anthropic_stream_parts(sse);
        assert_eq!(text, "Hi there");
        assert_eq!(finish, "stop");
        assert_eq!(usage, (5, 2));
        assert!(saw_done);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_anthropic_stream_max_tokens_maps_to_length() {
        let sse = concat!(
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"teil\"}}\n\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let (text, finish, _usage, saw_done, _tools) = anthropic_stream_parts(sse);
        assert_eq!(text, "teil");
        assert_eq!(finish, "length");
        assert!(saw_done);
    }

    #[tokio::test]
    async fn test_models_list_401_counts_auth_failure() {
        // Lazy-Konnektionen: der 401-Pfad wird vor jeglichem DB-Zugriff
        // erreicht, die Pools werden nur konstruiert, nie angeruehrt.
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@127.0.0.1:1/none")
            .unwrap();
        let (sink, _ingest_handle) = ingest::start(
            clickhouse::Client::default(),
            ingest::IngestConfig::default(),
        );
        let state: AppState = Arc::new(common::state::AppStateInner::new(
            pg,
            clickhouse::Client::default(),
            reqwest::Client::new(),
            sink.clone(),
            "session-secret".into(),
            "0".repeat(32),
            Arc::new(tokio::sync::RwLock::new(
                common::state::MetricsSnapshot::default(),
            )),
            None,
        ));

        let resp = models_list(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let (_, auth_failures) = sink.snapshot_counters();
        assert_eq!(auth_failures, 1);
    }

    #[test]
    fn test_detect_in_band_openai_error_event() {
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n",
            "data: {\"error\":{\"message\":\"rate limit exceeded\",\"type\":\"server_error\"}}\n\n",
            "data: [DONE]\n\n",
        );
        let err = detect_in_band_error(sse, StreamFormat::OpenAi).unwrap();
        assert!(err.contains("rate limit exceeded"), "{err}");
    }

    #[test]
    fn test_detect_in_band_openai_no_false_positive() {
        // "error" als normales wort im content: kein match
        let sse =
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"fix the error in code\"}}]}\n\n";
        assert!(detect_in_band_error(sse, StreamFormat::OpenAi).is_none());
        // quoted "error" in einem content-string: pre-filter trifft, der
        // top-level-key-check muss es trotzdem verwerfen
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"key \\\"error\\\" not set\"}}]}\n\n";
        assert!(detect_in_band_error(sse, StreamFormat::OpenAi).is_none());
        // sauberes transcript
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        assert!(detect_in_band_error(sse, StreamFormat::OpenAi).is_none());
        // schärfster fall: "error" als json-wert (tool-call-function-name)
        let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"function\":{\"name\":\"error\"}}]}}]}\n\n";
        assert!(detect_in_band_error(sse, StreamFormat::OpenAi).is_none());
    }

    #[test]
    fn test_detect_in_band_anthropic_error_event() {
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        );
        let err = detect_in_band_error(sse, StreamFormat::Anthropic).unwrap();
        assert!(err.contains("Overloaded"), "{err}");
    }

    #[test]
    fn test_detect_in_band_anthropic_clean() {
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        assert!(detect_in_band_error(sse, StreamFormat::Anthropic).is_none());
    }

    #[test]
    fn test_error_event_message_shapes() {
        // komplettes anthropic fehler-event
        let full = json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}});
        assert!(error_event_message(&full).contains("Overloaded"));
        // openai-artiges fehler-objekt
        let obj = json!({"error":{"message":"boom","type":"server_error"}});
        assert!(error_event_message(&obj).contains("boom"));
        // keine message: fallback auf type
        let t = error_event_message(&json!({"error":{"type":"rate_limited"}}));
        assert!(t.contains("rate_limited"));
        // nix nuetzliches: generisch
        assert_eq!(error_event_message(&json!({"error":{}})), "upstream stream error event");
    }

    #[test]
    fn test_gemini_in_band_error() {
        assert!(gemini_in_band_error(&json!({"error":{"code":503,"message":"Unavailable"}})));
        assert!(!gemini_in_band_error(&json!({"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]})));
        assert!(!gemini_in_band_error(&json!({"error":null})));
    }

    #[test]
    fn test_apply_stream_health_transport_wins_over_in_band() {
        let h = StreamHealth::new();
        h.set_in_band_error("in-band".to_string());
        h.set_in_band_error("second".to_string()); // erstes gewinnt
        h.set_transport_error("connection reset".to_string());
        let (status, et, em) = apply_stream_health(&h, "", StreamFormat::OpenAi, false).unwrap();
        // transport gewinnt bei error_type, aber die in-band-begruendung bleibt
        assert_eq!((status, et.as_str()), (502, "provider_network"));
        assert_eq!(em, "connection reset; in-band: in-band");
        assert_eq!(h.in_band_error().as_deref(), Some("in-band"));
    }

    #[test]
    fn test_apply_stream_health_in_band_and_none() {
        let h = StreamHealth::new();
        assert!(apply_stream_health(&h, "data: [DONE]\n\n", StreamFormat::OpenAi, true).is_none());
        h.set_in_band_error("upstream stream error: Overloaded".to_string());
        let (status, et, em) = apply_stream_health(&h, "", StreamFormat::OpenAi, false).unwrap();
        assert_eq!(status, 502);
        assert_eq!(et, "provider_error");
        assert!(em.contains("Overloaded"));
    }

    #[test]
    fn test_apply_stream_health_skips_transcript_scan_when_disabled() {
        // transcript enthaelt ein fehler-event, aber scan aus = kein override
        // (konvertierte pfaede: fehler-events sind dort nie im transcript)
        let h = StreamHealth::new();
        let sse = "data: {\"error\":{\"message\":\"boom\"}}\n\n";
        assert!(apply_stream_health(&h, sse, StreamFormat::OpenAi, false).is_none());
        assert!(apply_stream_health(&h, sse, StreamFormat::OpenAi, true).is_some());
    }

    #[tokio::test]
    async fn test_drain_log_rx_stays_open_beyond_limit() {
        // sender-task sendet > 256 KB, setzt DANN einen transport-fehler in
        // health und droppt erst danach den sender. der drain muss bis zum
        // drop pending bleiben (alter collector ist bei der grenze
        // ausgestiegen und wuerde den spaeten fehler verpassen).
        let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(16);
        let (sent_tx, sent_rx) = tokio::sync::oneshot::channel::<()>();
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let health = StreamHealth::new();
        let health_sender = health.clone();
        let sender = tokio::spawn(async move {
            let head = bytes::Bytes::from("HEAD");
            tx.send(head).await.unwrap();
            let part = bytes::Bytes::from(vec![b'x'; 4096]);
            for _ in 0..79 {
                tx.send(part.clone()).await.unwrap();
            }
            tx.send(bytes::Bytes::from("TAIL")).await.unwrap(); // > 256 KB total
            sent_tx.send(()).unwrap();
            go_rx.await.unwrap();
            health_sender.set_transport_error("late transport error".to_string());
            drop(tx);
        });
        let mut task = tokio::spawn(async move { drain_log_rx(rx).await });
        sent_rx.await.unwrap();
        // channel ist noch offen: drain muss pending sein (race-frei per
        // oneshot synchronisiert, kein timing-glueck)
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut task)
                .await
                .is_err(),
            "drain must stay pending while log_tx is open"
        );
        go_tx.send(()).unwrap();
        sender.await.unwrap();
        let full = task.await.unwrap();
        // tail-semantik: nur die neusten 256 KB bleiben, der anfang ist weg
        assert!(full.len() <= 256 * 1024);
        assert!(!full.starts_with(b"HEAD"));
        assert!(full.ends_with(b"TAIL"));
        // spaeter fehler ist nach dem drain sichtbar
        assert_eq!(health.transport_error().as_deref(), Some("late transport error"));
    }
}
