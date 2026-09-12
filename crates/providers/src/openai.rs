//! OpenAI- und OpenAI-kompatibler Adapter (OpenAI, Azure-kompatibel via base_url,
//! Ollama, Groq, DeepSeek, Mistral, Together etc.).

use crate::{
    ProviderError, ProviderResponse, RouteTarget,
};
use serde_json::{json, Value};

pub struct OpenAiAdapter;

impl OpenAiAdapter {
    /// Baut die Upstream-URL fuer einen gegebenen Gateway-Endpunkt.
    pub fn endpoint_url(base_url: &str, endpoint: &str) -> String {
        let base = base_url.trim_end_matches('/');
        match endpoint {
            "/v1/chat/completions" => format!("{base}/chat/completions"),
            "/v1/embeddings" => format!("{base}/embeddings"),
            "/v1/models" => format!("{base}/models"),
            _ => format!("{base}/chat/completions"),
        }
    }

    /// Fuehrt einen nicht-streaming Request aus.
    pub async fn execute(
        client: &reqwest::Client,
        target: &RouteTarget,
        endpoint: &str,
        body: &Value,
    ) -> Result<ProviderResponse, ProviderError> {
        let url = Self::endpoint_url(&target.base_url, endpoint);
        // model-name auf upstream-model mappen
        let mut body = body.clone();
        body["model"] = json!(target.upstream_model);

        let resp = client
            .post(&url)
            .bearer_auth(&target.api_key)
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

    /// Extrahiert Usage aus einer OpenAI-Response.
    pub fn extract_usage(resp: &Value) -> (u64, u64) {
        let p = resp.pointer("/usage/prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let c = resp.pointer("/usage/completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        (p, c)
    }
}
