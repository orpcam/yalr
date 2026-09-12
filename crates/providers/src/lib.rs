pub mod anthropic;
pub mod gemini;
pub mod openai;
pub mod translate;

use std::collections::HashMap;

/// Art des Providers (aus DB `providers.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    Gemini,
    OpenAiCompat,
}

impl ProviderKind {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s {
            "openai" => Ok(Self::OpenAi),
            "anthropic" => Ok(Self::Anthropic),
            "gemini" => Ok(Self::Gemini),
            "openai_compat" => Ok(Self::OpenAiCompat),
            other => Err(anyhow::anyhow!("unbekannter provider kind: {other}")),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::OpenAiCompat => "openai_compat",
        }
    }
}

/// Ein konkretes Ziel (Provider + Modell), an das ein Request geroutet wird.
#[derive(Debug, Clone)]
pub struct RouteTarget {
    pub provider_id: uuid::Uuid,
    pub provider_name: String,
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub model_name: String,
    pub upstream_model: String,
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
    /// Freie modell-metadata (attachment, modalities, max_content_length, ...)
    pub capabilities: Option<serde_json::Value>,
}

/// Ergebnis einer Provider-Anfrage.
pub struct ProviderResponse {
    pub status: reqwest::StatusCode,
    pub headers: HashMap<String, String>,
    pub body: bytes::Bytes,
    pub is_stream: bool,
}

/// Fehler beim Provider-Call. `retryable` entscheidet ueber Retry/Fallback.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("provider returned error status {status}: {body}")]
    Status { status: u16, body: String },
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Network(_) => true,
            ProviderError::Status { status, .. } => {
                matches!(*status, 408 | 409 | 429 | 500 | 502 | 503 | 504)
            }
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            ProviderError::Network(_) => 502,
            ProviderError::Status { status, .. } => *status,
        }
    }
}

/// Berechnet Kosten in USD aus Token-Zahlen und Preisen (USD pro 1M Tokens).
pub fn compute_cost(
    prompt_tokens: u64,
    completion_tokens: u64,
    input_price_per_million: f64,
    output_price_per_million: f64,
) -> f64 {
    (prompt_tokens as f64 / 1_000_000.0) * input_price_per_million
        + (completion_tokens as f64 / 1_000_000.0) * output_price_per_million
}

/// Vereinfachte Token-Schaetzung, wenn der Provider keine Usage liefert
/// (~4 Zeichen pro Token, wie bei OpenAI-Modellen ueblich).
pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as f64 / 4.0).ceil() as u64
}

pub mod prelude {
    pub use super::anthropic::AnthropicAdapter;
    pub use super::gemini::GeminiAdapter;
    pub use super::openai::OpenAiAdapter;
    pub use super::{ProviderError, ProviderKind, ProviderResponse, RouteTarget};
}
