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
        translate::openai_request_to_anthropic(openai_req, &target.upstream_model)
    }

    /// Anthropic-Response in OpenAI-Format uebersetzen.
    pub fn translate_response(anthropic_resp: &Value, model_name: &str) -> Value {
        translate::anthropic_response_to_openai(anthropic_resp, model_name, chrono::Utc::now().timestamp())
    }

    /// Usage aus Anthropic-Response extrahieren (input, output).
    pub fn extract_usage(resp: &Value) -> (u64, u64) {
        let p = resp.pointer("/usage/input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let c = resp.pointer("/usage/output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        (p, c)
    }
}
