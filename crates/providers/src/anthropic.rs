//! Anthropic-Adapter: nativer Passthrough fuer `/v1/messages` plus Uebersetzung
//! von OpenAI-Format-Requests nach Anthropic (mit Streaming).

use crate::{
    translate,
    ProviderError, ProviderResponse, RouteTarget,
};
use serde_json::{json, Value};

pub struct AnthropicAdapter;

impl AnthropicAdapter {
    pub fn messages_url(base_url: &str) -> String {
        format!("{}/v1/messages", base_url.trim_end_matches('/'))
    }

    /// Fuehrt einen nicht-streaming Anthropic-Request aus.
    /// `openai_body` ist der originale OpenAI-Format-Request (wird uebersetzt);
    /// `native_body` ist bereits Anthropic-Format (fuer /v1/messages passthrough).
    pub async fn execute(
        client: &reqwest::Client,
        target: &RouteTarget,
        body: &Value,
    ) -> Result<ProviderResponse, ProviderError> {
        let mut body = body.clone();
        body["model"] = json!(target.upstream_model);

        let resp = client
            .post(Self::messages_url(&target.base_url))
            .header("x-api-key", &target.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            return Err(ProviderError::Status {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).to_string(),
            });
        }
        Ok(ProviderResponse {
            status,
            headers,
            body: bytes,
            is_stream: false,
        })
    }

    /// OpenAI-Format-Request in Anthropic-Format uebersetzen.
    pub fn translate_request(openai_req: &Value, target: &RouteTarget) -> Value {
        translate::openai_request_to_anthropic_mit(openai_req, &target.upstream_model, target.capabilities.as_ref())
    }

    /// Anthropic-Response in OpenAI-Format uebersetzen.
    pub fn translate_response(anthropic_resp: &Value, model_name: &str) -> Value {
        translate::anthropic_response_to_openai(anthropic_resp, model_name, chrono::Utc::now().timestamp())
    }

    /// Usage aus Anthropic-Response extrahieren (input, output). Input in
    /// basispreis-einheiten ([eingabe_bepreist]), damit die kosten bei prompt
    /// caching stimmen.
    pub fn extract_usage(resp: &Value, caps: Option<&Value>) -> (u64, u64) {
        let u = resp.get("usage").cloned().unwrap_or(Value::Null);
        let c = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        (eingabe_bepreist(&u, cache_faktoren(caps)), c)
    }
}

/// Eingabe-tokens einer anthropic-`usage`: (ungecacht, cache geschrieben, cache gelesen).
/// `input_tokens` zaehlt bei anthropic nur den ungecachten teil.
pub fn eingabe(u: &Value) -> (u64, u64, u64) {
    let n = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    (n("input_tokens"), n("cache_creation_input_tokens"), n("cache_read_input_tokens"))
}

/// Alle eingabe-tokens (ungecacht + geschrieben + gelesen) – so viele stehen im prompt.
pub fn eingabe_gesamt(u: &Value) -> u64 {
    let (i, w, r) = eingabe(u);
    i + w + r
}

/// Preisfaktoren fuer gecachte eingabe relativ zum eingabepreis: (schreiben, lesen).
/// Standard: schreiben 1,25 (bei `prompt_cache_ttl: "1h"` 2,0), lesen 0,1. Je modell
/// ueberschreibbar mit `cache_write_factor` / `cache_read_factor` in den capabilities,
/// weil die rabatte je modellgeneration verschieden sind.
pub fn cache_faktoren(caps: Option<&Value>) -> (f64, f64) {
    let zahl = |k: &str| caps.and_then(|c| c.get(k)).and_then(|v| v.as_f64()).filter(|x| *x >= 0.0);
    let lang = caps.and_then(|c| c.get("prompt_cache_ttl")).and_then(|v| v.as_str()) == Some("1h");
    (
        zahl("cache_write_factor").unwrap_or(if lang { 2.0 } else { 1.25 }),
        zahl("cache_read_factor").unwrap_or(0.1),
    )
}

/// Eingabe in basispreis-einheiten fuer die kosten: ungecacht voll, cache schreiben
/// und lesen mit ihren faktoren ([cache_faktoren]).
pub fn eingabe_bepreist(u: &Value, (schreiben, lesen): (f64, f64)) -> u64 {
    let (i, w, r) = eingabe(u);
    i + (w as f64 * schreiben + r as f64 * lesen).round() as u64
}

/// OpenAI-`usage` aus einer anthropic-`usage`: prompt_tokens = alle eingabe-tokens,
/// davon gecacht in `prompt_tokens_details` (cached_tokens = gelesen,
/// cache_creation_tokens = geschrieben).
pub fn usage_als_openai(u: &Value) -> Value {
    let (_, w, r) = eingabe(u);
    let p = eingabe_gesamt(u);
    let c = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let mut out = json!({ "prompt_tokens": p, "completion_tokens": c, "total_tokens": p + c });
    if w > 0 || r > 0 {
        out["prompt_tokens_details"] = json!({ "cached_tokens": r, "cache_creation_tokens": w });
    }
    out
}
