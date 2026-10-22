//! Konfiguration einer Anfrage fuer Dashboard und Log: welche Denkstufe und
//! welche Stellschrauben (Temperatur, max. Tokens, Werkzeuge, JSON-Modus) beim
//! Anbieter ankommen.
//!
//! Die Denkstufe steht je nach API an anderer Stelle und wird vereinheitlicht:
//! - `reasoning_effort` (OpenAI-kompatibel, vLLM)
//! - `reasoning.effort` / `reasoning.max_tokens` / `reasoning.enabled` (OpenRouter)
//! - `thinking` + `output_config.effort` (Anthropic)
//! - `chat_template_kwargs.enable_thinking` / `.reasoning_effort` (vLLM-Chat-Templates)
//! - `generationConfig.thinkingConfig` (Gemini)
//!
//! Schickt der Client keine Stufe und uebersetzt das Gateway fuer ein Modell mit
//! `capabilities.thinking: "adaptive"` nach Anthropic, gilt die `effort` aus den
//! Faehigkeiten – das ist als `effort_by_gateway` markiert.

use ingest::RequestConfig;
use providers::{ProviderKind, RouteTarget};
use serde_json::Value;

fn text<'a>(v: &'a Value, pfad: &str) -> Option<&'a str> {
    v.pointer(pfad).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())
}

fn zahl(v: &Value, pfad: &str) -> Option<u64> {
    v.pointer(pfad).and_then(|x| x.as_u64())
}

fn schalter(v: &Value, pfad: &str) -> Option<bool> {
    v.pointer(pfad).and_then(|x| x.as_bool())
}

/// Die Denkstufe, wie der Client sie angibt (leer = nicht angegeben).
fn denkstufe(req: &Value) -> String {
    if let Some(e) = text(req, "/reasoning_effort") {
        return e.to_string();
    }
    // openrouter
    if let Some(e) = text(req, "/reasoning/effort") {
        return e.to_string();
    }
    if let Some(n) = zahl(req, "/reasoning/max_tokens") {
        return format!("budget {n}");
    }
    if schalter(req, "/reasoning/enabled") == Some(false) {
        return "none".into();
    }
    // anthropic
    match text(req, "/thinking/type") {
        Some("disabled") => return "none".into(),
        Some("enabled") => {
            return zahl(req, "/thinking/budget_tokens").map(|n| format!("budget {n}")).unwrap_or_else(|| "on".into());
        }
        Some("adaptive") => {
            return text(req, "/output_config/effort").map(|e| format!("adaptive {e}")).unwrap_or_else(|| "adaptive".into());
        }
        _ => {}
    }
    if let Some(e) = text(req, "/output_config/effort") {
        return e.to_string();
    }
    // vllm-chat-templates (glm, qwen, …)
    if let Some(e) = text(req, "/chat_template_kwargs/reasoning_effort") {
        return e.to_string();
    }
    for k in ["/chat_template_kwargs/enable_thinking", "/chat_template_kwargs/thinking"] {
        match schalter(req, k) {
            Some(false) => return "none".into(),
            Some(true) => return "on".into(),
            None => {}
        }
    }
    // gemini
    if let Some(n) = zahl(req, "/generationConfig/thinkingConfig/thinkingBudget") {
        return if n == 0 { "none".into() } else { format!("budget {n}") };
    }
    if let Some(e) = text(req, "/generationConfig/thinkingConfig/thinkingLevel") {
        return e.to_lowercase();
    }
    String::new()
}

/// Die Konfiguration einer Anfrage an `endpoint`; `target` ist das (erste)
/// Ziel, fuer die Denkstufe, die das Gateway selbst setzt.
pub fn request_config(req: &Value, endpoint: &str, target: Option<&RouteTarget>) -> RequestConfig {
    let mut reasoning_effort = denkstufe(req);
    let mut effort_by_gateway = false;
    // wie providers::translate::nachdenken_setzen: nur bei der uebersetzung
    // openai -> anthropic (nicht am nativen /v1/messages)
    if reasoning_effort.is_empty() && endpoint != "/v1/messages" {
        if let Some(t) = target.filter(|t| t.provider_kind == ProviderKind::Anthropic) {
            let caps = t.capabilities.as_ref();
            let cap = |k: &str| caps.and_then(|c| c.get(k)).and_then(|v| v.as_str());
            if cap("thinking") == Some("adaptive") {
                if let Some(e) = cap("effort") {
                    reasoning_effort = e.to_string();
                    effort_by_gateway = true;
                }
            }
        }
    }
    let max_tokens = ["/max_completion_tokens", "/max_tokens", "/max_output_tokens", "/generationConfig/maxOutputTokens"]
        .iter()
        .find_map(|p| zahl(req, p));
    let json_mode = matches!(text(req, "/response_format/type"), Some("json_object" | "json_schema"))
        || text(req, "/generationConfig/responseMimeType") == Some("application/json");
    RequestConfig {
        reasoning_effort,
        effort_by_gateway,
        temperature: req.get("temperature").and_then(|t| t.as_f64()),
        top_p: req.get("top_p").and_then(|t| t.as_f64()),
        max_tokens,
        tools: req.get("tools").and_then(|t| t.as_array()).map(|a| a.len() as u32).unwrap_or(0),
        json_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stufe(req: Value) -> String {
        request_config(&req, "/v1/chat/completions", None).reasoning_effort
    }

    #[test]
    fn effort_from_the_usual_places() {
        assert_eq!(stufe(json!({"reasoning_effort": "high"})), "high");
        assert_eq!(stufe(json!({"reasoning": {"effort": "max"}})), "max");
        assert_eq!(stufe(json!({"reasoning": {"max_tokens": 4000}})), "budget 4000");
        assert_eq!(stufe(json!({"reasoning": {"enabled": false}})), "none");
        assert_eq!(stufe(json!({"thinking": {"type": "enabled", "budget_tokens": 8000}})), "budget 8000");
        assert_eq!(stufe(json!({"thinking": {"type": "adaptive"}, "output_config": {"effort": "medium"}})), "adaptive medium");
        assert_eq!(stufe(json!({"thinking": {"type": "disabled"}})), "none");
        assert_eq!(stufe(json!({"chat_template_kwargs": {"enable_thinking": false}})), "none");
        assert_eq!(stufe(json!({"chat_template_kwargs": {"enable_thinking": true}})), "on");
        assert_eq!(stufe(json!({"generationConfig": {"thinkingConfig": {"thinkingBudget": 0}}})), "none");
        assert_eq!(stufe(json!({"generationConfig": {"thinkingConfig": {"thinkingLevel": "HIGH"}}})), "high");
        assert_eq!(stufe(json!({"messages": []})), "");
    }

    #[test]
    fn settings_and_json_column() {
        let c = request_config(
            &json!({"temperature": 0.7, "top_p": 0.9, "max_tokens": 32000, "tools": [{}, {}],
                    "response_format": {"type": "json_object"}, "reasoning_effort": "low"}),
            "/v1/chat/completions",
            None,
        );
        assert_eq!(
            c,
            RequestConfig {
                reasoning_effort: "low".into(),
                effort_by_gateway: false,
                temperature: Some(0.7),
                top_p: Some(0.9),
                max_tokens: Some(32000),
                tools: 2,
                json_mode: true,
            }
        );
        assert_eq!(
            c.als_json(),
            r#"{"reasoning_effort":"low","temperature":0.7,"top_p":0.9,"max_tokens":32000,"tools":2,"json_mode":true}"#
        );
        assert_eq!(request_config(&json!({"messages": []}), "/v1/chat/completions", None).als_json(), "");
    }

    #[test]
    fn effort_set_by_the_gateway_for_adaptive_models() {
        let t = RouteTarget {
            provider_id: uuid::Uuid::nil(),
            provider_name: "p".into(),
            provider_kind: ProviderKind::Anthropic,
            base_url: String::new(),
            api_key: String::new(),
            model_name: "m".into(),
            upstream_model: "m".into(),
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            capabilities: Some(json!({"thinking": "adaptive", "effort": "high"})),
        };
        let c = request_config(&json!({"messages": []}), "/v1/chat/completions", Some(&t));
        assert_eq!((c.reasoning_effort.as_str(), c.effort_by_gateway), ("high", true));
        // eine stufe vom client gewinnt
        let c = request_config(&json!({"reasoning_effort": "low"}), "/v1/chat/completions", Some(&t));
        assert_eq!((c.reasoning_effort.as_str(), c.effort_by_gateway), ("low", false));
        // nativer messages-endpunkt: das gateway setzt nichts
        let c = request_config(&json!({"messages": []}), "/v1/messages", Some(&t));
        assert_eq!(c.reasoning_effort, "");
    }
}
