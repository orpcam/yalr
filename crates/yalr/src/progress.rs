//! Live-Fortschritt eines Streams: zaehlt waehrend der Antwort mit, wie viele
//! Tokens schon erzeugt wurden (Text, Nachdenken, Tool-Argumente), und meldet
//! den Stand gedrosselt als `LiveEvent::Progress` an das Dashboard.
//!
//! Gezaehlt wird formatunabhaengig an den Stream-Events (OpenAI-Chunks,
//! Anthropic-Events, Gemini-Chunks). Meldet der Anbieter selbst eine Zahl
//! (`usage` im Chunk), gilt sie, sobald sie mindestens so gross ist wie die
//! Schaetzung; sonst schaetzt der Zaehler wie `estimate_tokens` (4 Bytes je
//! Token).

use std::time::{Duration, Instant};

use ingest::{LiveEvent, LogSink};
use serde_json::Value;

/// Hoechstens so oft geht ein Zwischenstand je Request an das Dashboard.
const MELDE_TAKT: Duration = Duration::from_secs(1);

/// Zaehler fuer einen laufenden Stream.
pub struct StreamProgress {
    request_id: String,
    /// erzeugter Text samt Nachdenken (Bytes)
    bytes: u64,
    /// davon Nachdenken (Bytes)
    reasoning_bytes: u64,
    /// vom Anbieter gemeldete Zahl der erzeugten Tokens (falls vorhanden)
    gemeldet: Option<u64>,
    /// angefangene SSE-Zeile aus dem letzten Byte-Stueck
    zeile: Vec<u8>,
    zuletzt: Option<Instant>,
    zuletzt_tokens: u64,
    /// wann das erste erzeugte zeichen kam (basis fuer tokens/s)
    erstes: Option<Instant>,
}

/// Stand fuer das Dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stand {
    pub tokens: u64,
    pub reasoning_tokens: u64,
    /// true, wenn `tokens` vom Anbieter stammt (sonst geschaetzt)
    pub exact: bool,
}

fn geschaetzt(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

fn text_len(v: Option<&Value>) -> u64 {
    v.and_then(|t| t.as_str()).map(|s| s.len() as u64).unwrap_or(0)
}

impl StreamProgress {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            bytes: 0,
            reasoning_bytes: 0,
            gemeldet: None,
            zeile: Vec::new(),
            zuletzt: None,
            zuletzt_tokens: 0,
            erstes: None,
        }
    }

    /// Rohe SSE-Bytes einspeisen (Events duerfen ueber Stuecke verteilt sein).
    pub fn feed(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if b == b'\n' {
                let zeile = std::mem::take(&mut self.zeile);
                self.zeile_lesen(&zeile);
            } else {
                self.zeile.push(b);
            }
        }
    }

    fn zeile_lesen(&mut self, zeile: &[u8]) {
        let Ok(text) = std::str::from_utf8(zeile) else { return };
        let Some(data) = text.trim_end_matches('\r').strip_prefix("data:") else { return };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        if let Ok(v) = serde_json::from_str::<Value>(data) {
            self.observe(&v);
        }
    }

    /// Ein schon gelesenes Event zaehlen (OpenAI-, Anthropic- oder Gemini-Form).
    pub fn observe(&mut self, v: &Value) {
        // openai: choices[].delta
        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
            for c in choices {
                let Some(d) = c.get("delta") else { continue };
                let reasoning = text_len(d.get("reasoning_content")) + text_len(d.get("reasoning"));
                self.reasoning_bytes += reasoning;
                self.bytes += reasoning + text_len(d.get("content"));
                if let Some(calls) = d.get("tool_calls").and_then(|t| t.as_array()) {
                    for call in calls {
                        self.bytes += text_len(call.pointer("/function/arguments"));
                    }
                }
            }
            if let Some(n) = v.pointer("/usage/completion_tokens").and_then(|n| n.as_u64()) {
                self.gemeldet = Some(n);
            }
            return;
        }
        // anthropic: content_block_delta / message_delta
        match v.get("type").and_then(|t| t.as_str()) {
            Some("content_block_delta") => {
                let d = v.get("delta");
                let thinking = text_len(d.and_then(|d| d.get("thinking")));
                self.reasoning_bytes += thinking;
                self.bytes += thinking
                    + text_len(d.and_then(|d| d.get("text")))
                    + text_len(d.and_then(|d| d.get("partial_json")));
                return;
            }
            Some("message_delta") => {
                if let Some(n) = v.pointer("/usage/output_tokens").and_then(|n| n.as_u64()) {
                    self.gemeldet = Some(n);
                }
                return;
            }
            Some(_) => return,
            None => {}
        }
        // gemini: candidates[].content.parts[] (thought: true = nachdenken)
        if let Some(cands) = v.get("candidates").and_then(|c| c.as_array()) {
            for c in cands {
                let Some(parts) = c.pointer("/content/parts").and_then(|p| p.as_array()) else { continue };
                for p in parts {
                    let n = text_len(p.get("text"));
                    if p.get("thought").and_then(|t| t.as_bool()) == Some(true) {
                        self.reasoning_bytes += n;
                    }
                    self.bytes += n;
                }
            }
        }
        if let Some(u) = v.get("usageMetadata") {
            let kandidaten = u.get("candidatesTokenCount").and_then(|n| n.as_u64());
            let gedanken = u.get("thoughtsTokenCount").and_then(|n| n.as_u64());
            if kandidaten.is_some() || gedanken.is_some() {
                self.gemeldet = Some(kandidaten.unwrap_or(0) + gedanken.unwrap_or(0));
            }
        }
    }

    pub fn stand(&self) -> Stand {
        let schaetzung = geschaetzt(self.bytes);
        let reasoning_tokens = geschaetzt(self.reasoning_bytes);
        match self.gemeldet {
            Some(n) if n > 0 && n >= schaetzung => Stand { tokens: n, reasoning_tokens, exact: true },
            _ => Stand { tokens: schaetzung, reasoning_tokens, exact: false },
        }
    }

    /// Stand melden, wenn sich etwas getan hat und der letzte Zwischenstand
    /// mindestens [`MELDE_TAKT`] her ist.
    pub fn melden(&mut self, sink: &LogSink) {
        self.melden_um(sink, Instant::now());
    }

    fn melden_um(&mut self, sink: &LogSink, jetzt: Instant) {
        let stand = self.stand();
        if stand.tokens > 0 && self.erstes.is_none() {
            self.erstes = Some(jetzt);
        }
        if stand.tokens == self.zuletzt_tokens {
            return;
        }
        if self.zuletzt.is_some_and(|t| jetzt.duration_since(t) < MELDE_TAKT) {
            return;
        }
        self.zuletzt = Some(jetzt);
        self.zuletzt_tokens = stand.tokens;
        // tokens/s erst ab einer sekunde seit dem ersten zeichen (sonst zu zappelig)
        let tokens_per_s = self.erstes.and_then(|t| {
            let s = jetzt.duration_since(t).as_secs_f64();
            (s >= 1.0).then(|| stand.tokens as f64 / s)
        });
        sink.emit(LiveEvent::Progress {
            request_id: self.request_id.clone(),
            generated_tokens: stand.tokens,
            reasoning_tokens: stand.reasoning_tokens,
            tokens_exact: stand.exact,
            tokens_per_s,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_chunks_across_pieces() {
        let mut p = StreamProgress::new("r");
        let a = "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"abcdefgh\"}}]}\n\ndata: {\"choi";
        let b = "ces\":[{\"delta\":{\"content\":\"1234\"}}]}\r\n\r\ndata: [DONE]\n\n";
        p.feed(a.as_bytes());
        assert_eq!(p.stand(), Stand { tokens: 2, reasoning_tokens: 2, exact: false });
        p.feed(b.as_bytes());
        assert_eq!(p.stand(), Stand { tokens: 3, reasoning_tokens: 2, exact: false });
    }

    #[test]
    fn openai_tool_arguments_and_reported_usage() {
        let mut p = StreamProgress::new("r");
        p.observe(&json!({"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"{\"a\":1}"}}]}}]}));
        assert_eq!(p.stand().tokens, 2);
        // die gemeldete zahl gilt, sobald sie die schaetzung erreicht
        p.observe(&json!({"choices":[],"usage":{"completion_tokens":40}}));
        assert_eq!(p.stand(), Stand { tokens: 40, reasoning_tokens: 0, exact: true });
    }

    #[test]
    fn reported_count_below_estimate_is_ignored() {
        let mut p = StreamProgress::new("r");
        p.observe(&json!({"type":"message_delta","usage":{"output_tokens":1}}));
        p.observe(&json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"x".repeat(40)}}));
        assert_eq!(p.stand(), Stand { tokens: 10, reasoning_tokens: 0, exact: false });
    }

    #[test]
    fn anthropic_events() {
        let mut p = StreamProgress::new("r");
        p.observe(&json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"12345678"}}));
        p.observe(&json!({"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"x\":"}}));
        p.observe(&json!({"type":"ping"}));
        assert_eq!(p.stand(), Stand { tokens: 4, reasoning_tokens: 2, exact: false });
        p.observe(&json!({"type":"message_delta","usage":{"output_tokens":12}}));
        assert_eq!(p.stand(), Stand { tokens: 12, reasoning_tokens: 2, exact: true });
    }

    #[test]
    fn gemini_chunks() {
        let mut p = StreamProgress::new("r");
        p.observe(&json!({"candidates":[{"content":{"parts":[{"text":"abcd","thought":true},{"text":"efgh"}]}}]}));
        assert_eq!(p.stand(), Stand { tokens: 2, reasoning_tokens: 1, exact: false });
        p.observe(&json!({"candidates":[],"usageMetadata":{"candidatesTokenCount":5,"thoughtsTokenCount":3}}));
        assert_eq!(p.stand(), Stand { tokens: 8, reasoning_tokens: 1, exact: true });
    }

    #[tokio::test]
    async fn reports_throttled_and_only_on_change() {
        let (sink, _handle) = ingest::start(clickhouse::Client::default(), ingest::IngestConfig::default());
        let mut rx = sink.subscribe();
        let mut p = StreamProgress::new("r1");
        let t0 = Instant::now();
        p.melden_um(&sink, t0); // noch nichts erzeugt: keine meldung
        p.observe(&json!({"choices":[{"delta":{"content":"abcd"}}]}));
        p.melden_um(&sink, t0);
        p.observe(&json!({"choices":[{"delta":{"content":"efgh"}}]}));
        p.melden_um(&sink, t0 + Duration::from_millis(500)); // zu frueh
        p.melden_um(&sink, t0 + Duration::from_millis(1000));
        p.observe(&json!({"choices":[{"delta":{"content":"ijkl"}}]}));
        p.melden_um(&sink, t0 + Duration::from_millis(2000));
        let mut gemeldet = Vec::new();
        while let Ok(LiveEvent::Progress { generated_tokens, tokens_per_s, .. }) = rx.try_recv() {
            gemeldet.push((generated_tokens, tokens_per_s));
        }
        // 2 tokens nach 1 s, 3 nach 2 s; in der ersten sekunde keine rate
        assert_eq!(gemeldet, vec![(1, None), (2, Some(2.0)), (3, Some(1.5))]);
    }
}
