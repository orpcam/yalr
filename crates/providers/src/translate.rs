//! Format-Uebersetzung zwischen OpenAI-, Anthropic- und Gemini-Request/Response-Formaten.
//!
//! Das Gateway akzeptiert Anfragen im OpenAI-Format (und nativ Anthropic via
//! `/v1/messages`) und uebersetzt sie transparent in das Zielformat des Providers.

use serde_json::{json, Value};

// ============================================================
// OpenAI -> Anthropic
// ============================================================

/// OpenAI chat.completions request -> Anthropic messages request.
pub fn openai_request_to_anthropic(openai_req: &Value, upstream_model: &str) -> Value {
    let mut messages = Vec::new();
    let mut system = Vec::new();

    if let Some(msgs) = openai_req.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content").unwrap_or(&Value::Null);
            match role {
                "system" | "developer" => {
                    if let Some(text) = content_as_text(content) {
                        system.push(text);
                    }
                }
                "assistant" => {
                    let mut blocks = Vec::new();
                    if let Some(tool_calls) =
                        msg.get("tool_calls").and_then(|t| t.as_array())
                    {
                        for tc in tool_calls {
                            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let func = tc.get("function").unwrap_or(&Value::Null);
                            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let args = func
                                .get("arguments")
                                .and_then(|v| v.as_str())
                                .and_then(parse_json_lenient)
                                .unwrap_or_else(|| json!({}));
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": args,
                            }));
                        }
                    }
                    if let Some(text) = content_as_text(content) {
                        if !text.is_empty() {
                            blocks.push(json!({ "type": "text", "text": text }));
                        }
                    }
                    if !blocks.is_empty() {
                        messages.push(json!({ "role": "assistant", "content": blocks }));
                    }
                }
                "tool" => {
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or(""),
                            "content": content_as_text(content).unwrap_or_default(),
                        }],
                    }));
                }
                _ => {
                    // user
                    if let Some(arr) = content.as_array() {
                        let blocks: Vec<Value> = arr
                            .iter()
                            .filter_map(|part| {
                                let ty = part.get("type").and_then(|t| t.as_str())?;
                                match ty {
                                    "text" => Some(json!({
                                        "type": "text",
                                        "text": part.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                                    })),
                                    "image_url" => {
                                        let url = part
                                            .get("image_url")
                                            .and_then(|u| u.get("url"))
                                            .and_then(|u| u.as_str())
                                            .unwrap_or("");
                                        media_type_from_data_url(url).map(|(mt, data)| json!({
                                            "type": "image",
                                            "source": { "type": "base64", "media_type": mt, "data": data },
                                        }))
                                    }
                                    _ => None,
                                }
                            })
                            .collect();
                        if !blocks.is_empty() {
                            messages.push(json!({ "role": "user", "content": blocks }));
                        }
                    } else if let Some(text) = content_as_text(content) {
                        messages.push(json!({ "role": "user", "content": text }));
                    }
                }
            }
        }
    }

    let mut anthropic_req = json!({
        "model": upstream_model,
        "messages": messages,
        "max_tokens": openai_req.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(4096),
    });

    if !system.is_empty() {
        anthropic_req["system"] = json!(system.join("\n\n"));
    }

    if let Some(temp) = openai_req.get("temperature") {
        if !temp.is_null() {
            anthropic_req["temperature"] = temp.clone();
        }
    }
    if let Some(top_p) = openai_req.get("top_p") {
        if !top_p.is_null() {
            anthropic_req["top_p"] = top_p.clone();
        }
    }
    if let Some(stop) = openai_req.get("stop").and_then(|v| v.as_array()) {
        let stops: Vec<&str> = stop.iter().filter_map(|s| s.as_str()).collect();
        if !stops.is_empty() {
            anthropic_req["stop_sequences"] = json!(stops);
        }
    } else if let Some(stop) = openai_req.get("stop").and_then(|v| v.as_str()) {
        anthropic_req["stop_sequences"] = json!([stop]);
    }

    // tools -> Anthropic tool schema
    if let Some(tools) = openai_req.get("tools").and_then(|t| t.as_array()) {
        let a_tools: Vec<Value> = tools
            .iter()
            .filter_map(|t| {
                let func = t.get("function")?;
                Some(json!({
                    "name": func.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "description": func.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    "input_schema": func.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"})),
                }))
            })
            .collect();
        if !a_tools.is_empty() {
            anthropic_req["tools"] = json!(a_tools);
        }
    }

    // tool_choice
    if let Some(tc) = openai_req.get("tool_choice") {
        match tc.get("type").and_then(|t| t.as_str()) {
            Some("auto") => anthropic_req["tool_choice"] = json!({ "type": "auto" }),
            Some("none") => {
                anthropic_req.as_object_mut().map(|o| o.remove("tools"));
            }
            Some("required") => anthropic_req["tool_choice"] = json!({ "type": "any" }),
            Some("function") => {
                if let Some(name) = tc.pointer("/function/name").and_then(|v| v.as_str()) {
                    anthropic_req["tool_choice"] = json!({ "type": "tool", "name": name });
                }
            }
            _ => {}
        }
    }

    anthropic_req
}

/// Anthropic non-stream response -> OpenAI chat.completions response.
pub fn anthropic_response_to_openai(
    anthropic_resp: &Value,
    model_name: &str,
    created: i64,
) -> Value {
    let mut content_parts: Vec<Value> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut text = String::new();

    if let Some(blocks) = anthropic_resp.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    let t = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    text.push_str(t);
                    content_parts.push(json!({ "type": "text", "text": t }));
                }
                Some("tool_use") => {
                    let idx = tool_calls.len() as u32;
                    tool_calls.push(json!({
                        "id": block.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                        "type": "function",
                        "function": {
                            "name": block.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                            "arguments": serde_json::to_string(
                                block.get("input").unwrap_or(&json!({}))
                            ).unwrap_or_else(|_| "{}".into()),
                        },
                    }));
                    let _ = idx;
                }
                _ => {}
            }
        }
    }

    let usage = anthropic_resp.get("usage").cloned().unwrap_or_else(|| json!({}));
    let prompt_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let completion_tokens = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);

    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { json!(text) },
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }

    let mut resp = json!({
        "id": anthropic_resp.get("id").and_then(|v| v.as_str()).unwrap_or(""),
        "object": "chat.completion",
        "created": created,
        "model": model_name,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": anthropic_stop_reason_to_openai(
                anthropic_resp.get("stop_reason").and_then(|v| v.as_str())
            ),
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        },
    });

    // Anthropic-specific erweiterungen unter keys.x-namespace
    resp["keys"] = json!({ "anthropic": anthropic_resp });

    let _ = &mut content_parts;
    resp
}

fn anthropic_stop_reason_to_openai(stop: Option<&str>) -> &'static str {
    match stop {
        Some("end_turn") | Some("stop_sequence") => "stop",
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    }
}

/// Anthropic SSE event -> OpenAI chat.completion.chunk (falls moeglich).
/// Rueckgabe: Some(chunk) falls das Event in einen Chunk uebersetzt wurde.
pub fn anthropic_stream_event_to_openai(event: &str, data: &Value, model_name: &str) -> Option<Value> {
    match event {
        "content_block_start" => {
            let block = data.get("content_block")?;
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                let idx = data.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                return Some(json!({
                    "id": data.get("message").and_then(|m| m.get("id")).and_then(|i| i.as_str()).unwrap_or(""),
                    "object": "chat.completion.chunk",
                    "model": model_name,
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": idx,
                                "id": block.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                                "type": "function",
                                "function": { "name": block.get("name").and_then(|v| v.as_str()).unwrap_or(""), "arguments": "" },
                            }],
                        },
                    }],
                }));
            }
            None
        }
        "content_block_delta" => {
            let delta = data.get("delta")?;
            match delta.get("type").and_then(|t| t.as_str()) {
                Some("text_delta") => Some(json!({
                    "id": data.get("_message_id").and_then(|i| i.as_str()).unwrap_or(""),
                    "object": "chat.completion.chunk",
                    "model": model_name,
                    "choices": [{
                        "index": 0,
                        "delta": { "content": delta.get("text").and_then(|t| t.as_str()).unwrap_or("") },
                    }],
                })),
                Some("input_json_delta") => {
                    let idx = data.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                    Some(json!({
                        "id": "",
                        "object": "chat.completion.chunk",
                        "model": model_name,
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": idx,
                                    "function": { "arguments": delta.get("partial_json").and_then(|p| p.as_str()).unwrap_or("") },
                                }],
                            },
                        }],
                    }))
                }
                _ => None,
            }
        }
        "message_start" => {
            // message_id fuer nachfolgende chunks unter _message_id ablegen passiert im adapter;
            // hier: leerer initial-chunk mit role.
            let msg = data.get("message")?;
            Some(json!({
                "id": msg.get("id").and_then(|i| i.as_str()).unwrap_or(""),
                "object": "chat.completion.chunk",
                "model": model_name,
                "choices": [{
                    "index": 0,
                    "delta": { "role": "assistant", "content": "" },
                }],
            }))
        }
        "message_delta" => {
            let mut chunk = json!({
                "id": data.get("_message_id").and_then(|i| i.as_str()).unwrap_or(""),
                "object": "chat.completion.chunk",
                "model": model_name,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": anthropic_stop_reason_to_openai(
                        data.get("delta").and_then(|d| d.get("stop_reason")).and_then(|s| s.as_str())
                    ),
                }],
            });
            if let Some(usage) = data.get("usage") {
                chunk["usage"] = json!({
                    "prompt_tokens": usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                    "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                    "total_tokens": usage
                        .get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                        + usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                });
            }
            Some(chunk)
        }
        _ => None,
    }
}

// ============================================================
// OpenAI -> Gemini
// ============================================================

/// OpenAI chat.completions request -> Gemini generateContent request.
pub fn openai_request_to_gemini(openai_req: &Value) -> Value {
    let mut contents = Vec::new();
    let mut system_instruction = Vec::new();

    if let Some(msgs) = openai_req.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg.get("content").unwrap_or(&Value::Null);
            match role {
                "system" | "developer" => {
                    if let Some(text) = content_as_text(content) {
                        system_instruction.push(text);
                    }
                }
                "assistant" => {
                    let mut parts = Vec::new();
                    if let Some(text) = content_as_text(content) {
                        parts.push(json!({ "text": text }));
                    }
                    if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tool_calls {
                            let func = tc.get("function").unwrap_or(&Value::Null);
                            let args = func
                                .get("arguments")
                                .and_then(|v| v.as_str())
                                .and_then(parse_json_lenient)
                                .unwrap_or_else(|| json!({}));
                            parts.push(json!({
                                "functionCall": {
                                    "name": func.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                    "args": args,
                                }
                            }));
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(json!({ "role": "model", "parts": parts }));
                    }
                }
                "tool" => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or(""),
                                "response": parse_json_lenient(&content_as_text(content).unwrap_or_default())
                                    .unwrap_or_else(|| json!({})),
                            },
                        }],
                    }));
                }
                _ => {
                    let mut parts = Vec::new();
                    if let Some(arr) = content.as_array() {
                        for part in arr {
                            match part.get("type").and_then(|t| t.as_str()) {
                                Some("text") => parts.push(json!({
                                    "text": part.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                                })),
                                Some("image_url") => {
                                    let url = part
                                        .get("image_url")
                                        .and_then(|u| u.get("url"))
                                        .and_then(|u| u.as_str())
                                        .unwrap_or("");
                                    if let Some((mt, data)) = media_type_from_data_url(url) {
                                        parts.push(json!({
                                            "inlineData": { "mimeType": mt, "data": data },
                                        }));
                                    }
                                }
                                _ => {}
                            }
                        }
                    } else if let Some(text) = content_as_text(content) {
                        parts.push(json!({ "text": text }));
                    }
                    if !parts.is_empty() {
                        contents.push(json!({ "role": "user", "parts": parts }));
                    }
                }
            }
        }
    }

    let mut gemini_req = json!({ "contents": contents });

    if !system_instruction.is_empty() {
        gemini_req["systemInstruction"] = json!({
            "parts": [{ "text": system_instruction.join("\n\n") }],
        });
    }

    let mut gen_config = serde_json::Map::new();
    if let Some(mt) = openai_req.get("max_tokens").and_then(|v| v.as_u64()) {
        gen_config.insert("maxOutputTokens".into(), json!(mt));
    }
    if let Some(t) = openai_req.get("temperature") {
        if !t.is_null() {
            gen_config.insert("temperature".into(), t.clone());
        }
    }
    if let Some(t) = openai_req.get("top_p") {
        if !t.is_null() {
            gen_config.insert("topP".into(), t.clone());
        }
    }
    if let Some(stop) = openai_req.get("stop") {
        let stops: Vec<Value> = match stop {
            Value::Array(a) => a.clone(),
            Value::String(s) => vec![json!(s)],
            _ => Vec::new(),
        };
        if !stops.is_empty() {
            gen_config.insert("stopSequences".into(), json!(stops));
        }
    }
    if let Some(tools) = openai_req.get("tools").and_then(|t| t.as_array()) {
        let mut decls = Vec::new();
        for t in tools {
            if let Some(func) = t.get("function") {
                decls.push(json!({
                    "name": func.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "description": func.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    "parameters": func.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"})),
                }));
            }
        }
        if !decls.is_empty() {
            gen_config.insert("tools".into(), json!([{ "functionDeclarations": decls }]));
        }
    }
    if let Some(tc) = openai_req.get("tool_choice") {
        match tc.get("type").and_then(|t| t.as_str()) {
            Some("none") => {
                gen_config.remove("tools");
            }
            Some("required") => {
                gen_config.insert("toolConfig".into(), json!({ "functionCallingConfig": { "mode": "ANY" } }));
            }
            Some("function") => {
                if let Some(name) = tc.pointer("/function/name").and_then(|v| v.as_str()) {
                    gen_config.insert(
                        "toolConfig".into(),
                        json!({ "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": [name] } }),
                    );
                }
            }
            _ => {}
        }
    }
    if !gen_config.is_empty() {
        gemini_req["generationConfig"] = Value::Object(gen_config);
    }

    gemini_req
}

/// Gemini generateContent response -> OpenAI chat.completions response.
pub fn gemini_response_to_openai(gemini_resp: &Value, model_name: &str, created: i64) -> Value {
    let mut text = String::new();
    let mut tool_calls = Vec::new();

    if let Some(candidates) = gemini_resp.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                for part in parts {
                    if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                    if let Some(fc) = part.get("functionCall") {
                        tool_calls.push(json!({
                            "id": format!("call_{}", fc.get("name").and_then(|n| n.as_str()).unwrap_or("")),
                            "type": "function",
                            "function": {
                                "name": fc.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                                "arguments": serde_json::to_string(
                                    fc.get("args").unwrap_or(&json!({}))
                                ).unwrap_or_else(|_| "{}".into()),
                            },
                        }));
                    }
                }
            }
        }
    }

    let usage = gemini_resp.get("usageMetadata").cloned().unwrap_or_else(|| json!({}));
    let prompt_tokens = usage.get("promptTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
    let completion_tokens = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);

    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { json!(text) },
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }

    json!({
        "id": format!("gemini-{}", created),
        "object": "chat.completion",
        "created": created,
        "model": model_name,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": if tool_calls.is_empty() { "stop" } else { "tool_calls" },
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        },
    })
}

/// Gemini streaming chunk -> OpenAI chat.completion.chunk.
pub fn gemini_chunk_to_openai(chunk: &Value, model_name: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let usage = chunk.get("usageMetadata").cloned().unwrap_or_else(|| json!({}));
    let prompt_tokens = usage.get("promptTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
    let completion_tokens = usage.get("candidatesTokenCount").and_then(|v| v.as_u64()).unwrap_or(0);
    let has_usage = !usage.is_null() && (prompt_tokens > 0 || completion_tokens > 0);

    if let Some(candidates) = chunk.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            let finish = candidate
                .get("finishReason")
                .and_then(|f| f.as_str())
                .map(gemini_finish_to_openai);
            if let Some(parts) = candidate.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
                for part in parts {
                    if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                        out.push(json!({
                            "id": "gemini-stream",
                            "object": "chat.completion.chunk",
                            "model": model_name,
                            "choices": [{
                                "index": 0,
                                "delta": { "content": t },
                            }],
                        }));
                    }
                    if let Some(fc) = part.get("functionCall") {
                        out.push(json!({
                            "id": "gemini-stream",
                            "object": "chat.completion.chunk",
                            "model": model_name,
                            "choices": [{
                                "index": 0,
                                "delta": {
                                    "tool_calls": [{
                                        "index": 0,
                                        "id": format!("call_{}", fc.get("name").and_then(|n| n.as_str()).unwrap_or("")),
                                        "type": "function",
                                        "function": {
                                            "name": fc.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                                            "arguments": serde_json::to_string(
                                                fc.get("args").unwrap_or(&json!({}))
                                            ).unwrap_or_else(|_| "{}".into()),
                                        },
                                    }],
                                },
                            }],
                        }));
                    }
                }
            }
            if let Some(reason) = finish {
                let mut c = json!({
                    "id": "gemini-stream",
                    "object": "chat.completion.chunk",
                    "model": model_name,
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": reason,
                    }],
                });
                if has_usage {
                    c["usage"] = json!({
                        "prompt_tokens": prompt_tokens,
                        "completion_tokens": completion_tokens,
                        "total_tokens": prompt_tokens + completion_tokens,
                    });
                }
                out.push(c);
            }
        }
    }
    out
}

fn gemini_finish_to_openai(reason: &str) -> &'static str {
    match reason {
        "MAX_TOKENS" => "length",
        "SAFETY" => "content_filter",
        "RECITATION" => "content_filter",
        _ => "stop",
    }
}

// ============================================================
// Helpers
// ============================================================

/// Extrahiert Text aus OpenAI-content (string oder parts-array).
pub fn content_as_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            if texts.is_empty() { None } else { Some(texts.join("\n")) }
        }
        _ => None,
    }
}

/// "data:image/png;base64,XXXX" -> ("image/png", "XXXX")
pub fn media_type_from_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    if let Some((meta, data)) = rest.split_once(",") {
        let mt = meta.strip_suffix(";base64")?;
        Some((mt.to_string(), data.to_string()))
    } else {
        None
    }
}

fn parse_json_lenient(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_to_anthropic_basic() {
        let req = json!({
            "model": "claude",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hi"},
                {"role": "assistant", "content": "Hello!"},
                {"role": "user", "content": "How are you?"},
            ],
            "max_tokens": 1024,
            "temperature": 0.5,
        });
        let out = openai_request_to_anthropic(&req, "claude-3-5-sonnet");
        assert_eq!(out["model"], "claude-3-5-sonnet");
        assert_eq!(out["system"], "You are helpful.");
        assert_eq!(out["max_tokens"], 1024);
        assert_eq!(out["temperature"], 0.5);
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[2]["role"], "user");
    }

    #[test]
    fn test_anthropic_to_openai_response() {
        let resp = json!({
            "id": "msg_123",
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5},
        });
        let out = anthropic_response_to_openai(&resp, "claude", 12345);
        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["choices"][0]["message"]["content"], "Hello!");
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
        assert_eq!(out["usage"]["total_tokens"], 15);
    }

    #[test]
    fn test_openai_to_gemini() {
        let req = json!({
            "model": "gemini",
            "messages": [
                {"role": "system", "content": "Be nice."},
                {"role": "user", "content": "Hi"},
            ],
            "max_tokens": 100,
        });
        let out = openai_request_to_gemini(&req);
        assert_eq!(out["systemInstruction"]["parts"][0]["text"], "Be nice.");
        assert_eq!(out["contents"][0]["role"], "user");
        assert_eq!(out["generationConfig"]["maxOutputTokens"], 100);
    }

    #[test]
    fn test_gemini_to_openai_response() {
        let resp = json!({
            "candidates": [{
                "content": {"parts": [{"text": "Hi there"}]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2},
        });
        let out = gemini_response_to_openai(&resp, "gemini", 1);
        assert_eq!(out["choices"][0]["message"]["content"], "Hi there");
        assert_eq!(out["usage"]["total_tokens"], 5);
    }

    #[test]
    fn test_data_url_parsing() {
        let (mt, data) = media_type_from_data_url("data:image/jpeg;base64,abc123").unwrap();
        assert_eq!(mt, "image/jpeg");
        assert_eq!(data, "abc123");
    }
}
