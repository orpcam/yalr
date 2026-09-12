//! Google Gemini-Adapter (generativelanguage.googleapis.com, v1beta).

use crate::{
    translate,
    ProviderError, ProviderResponse, RouteTarget,
};
use serde_json::Value;

pub struct GeminiAdapter;

impl GeminiAdapter {
    /// `generateContent` URL (stream=false) bzw. `streamGenerateContent` (stream=true).
    pub fn url(base_url: &str, upstream_model: &str, stream: bool, api_key: &str) -> String {
        let base = base_url.trim_end_matches('/');
        let method = if stream { "streamGenerateContent" } else { "generateContent" };
        format!("{base}/models/{upstream_model}:{method}?key={api_key}")
    }

    pub async fn execute(
        client: &reqwest::Client,
        target: &RouteTarget,
        openai_body: &Value,
    ) -> Result<ProviderResponse, ProviderError> {
        let gemini_body = translate::openai_request_to_gemini(openai_body);
        let url = Self::url(&target.base_url, &target.upstream_model, false, &target.api_key);

        let resp = client.post(&url).json(&gemini_body).send().await?;
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

    pub fn translate_response(gemini_resp: &Value, model_name: &str) -> Value {
        translate::gemini_response_to_openai(gemini_resp, model_name, chrono::Utc::now().timestamp())
    }
}
