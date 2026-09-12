//! Minimaler Prometheus-/metrics-Parser (Textformat) fuer provider-queue-metriken.
//!
//! Unterstuetzt die gaengigen metric-namen fuer queue/running-tiefen
//! (vLLM, TGI, SGLang, TabbyAPI u.a.):
//!   - vllm:num_requests_waiting           (queue-tiefe)
//!   - vllm:num_requests_running           (aktive requests)
//!   - tgi:queue_size / queue_size
//!   - tgi:num_requests_running / num_requests_running
//!   - sglang:num_queue_reqs               (queue-tiefe, SGLang)
//!   - sglang:num_running_reqs             (aktive requests, SGLang)
//!
//! Beispiel-zeile:  vllm:num_requests_waiting{model="qwen"} 3

use std::time::Instant;

/// Summiert einen gauge-wert aus prometheus-textformat (prometheus-semantik).
///
/// Mehrere gelabelte serien desselben metrics werden summiert (z.b. vLLM
/// pro model_name/instanz). NaN/inf-zeilen werden ignoriert, damit sie die
/// summe nicht vergiften.
///
/// Unlabeled-Zeilen desselben Metric-Namens werden separat akkumuliert (ebenfalls
/// summiert) und nur als Fallback verwertet, wenn keine gelabelte Serie existiert.
/// Falls sowohl gelabelte Serien als auch unlabeled-Zeilen exposiert werden,
/// zahlt nur die gelabelte Summe (kein Double-Count).
pub fn parse_gauge(body: &str, metric_name: &str) -> Option<f64> {
    let mut labeled: Option<f64> = None;
    let mut unlabeled: Option<f64> = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // format: name{labels} value [timestamp]  |  name value
        let (key_part, value_part) = match line.split_once(' ') {
            Some((k, v)) => (k, v),
            None => continue,
        };
        // metric-name = text vor '{' oder whole key
        let name = key_part.split('{').next().unwrap_or(key_part);
        if name != metric_name {
            continue;
        }
        // value = erstes token im value-part
        let Some(value_token) = value_part.split_whitespace().next() else {
            continue;
        };
        let Ok(v) = value_token.parse::<f64>() else {
            continue;
        };
        // nan/inf-guards: nicht-endliche werte koennen die summe nicht vergiften
        if !v.is_finite() {
            continue;
        }
        if key_part.contains('{') {
            labeled = Some(labeled.unwrap_or(0.0) + v);
        } else {
            unlabeled = Some(unlabeled.unwrap_or(0.0) + v);
        }
    }
    labeled.or(unlabeled)
}

/// Queue-tiefe + running-zaehler aus einem /metrics-body lesen.
/// Probiert mehrere bekannte metric-namen (vLLM, TGI, SGLang, generisch).
pub fn queue_metrics(body: &str) -> (Option<f64>, Option<f64>) {
    // Reihenfolge: vendor-spezifische namen zuerst (ein body stammt von genau
    // einem engine, die prefixes mischen sich nicht), generische fallbacks zuletzt.
    const QUEUE_NAMES: &[&str] = &[
        "vllm:num_requests_waiting",
        "vllm:num_requests_swapped",
        "tgi:queue_size",
        "sglang:num_queue_reqs",
        "queue_size",
        "num_requests_waiting",
    ];
    const RUNNING_NAMES: &[&str] = &[
        "vllm:num_requests_running",
        "tgi:num_requests_running",
        "sglang:num_running_reqs",
        "num_requests_running",
    ];

    let queued = QUEUE_NAMES.iter().find_map(|n| parse_gauge(body, n));
    let running = RUNNING_NAMES.iter().find_map(|n| parse_gauge(body, n));
    (queued, running)
}

/// KV-Cache-Auslastung (utilization ratio, 0..1) aus einem /metrics-body lesen.
///
/// Probiert mehrere bekannte metric-namen; Reihenfolge: vendor-spezifische
/// namen zuerst (ein body stammt von genau einem engine), erster Treffer
/// gewinnt. SGLang exportiert in dieser Version kein eigenes
/// `sglang:kv_cache_usage`; `sglang:token_usage` ist das von SGLang selbst als
/// "token usage %" geloggte Aequivalent. TGI exportiert in aktuellen
/// Versionen gar keine kv-cache-metrik, daher liefert die Funktion fuer TGI
/// korrekt `None` (statt einen nicht existierenden Namen zu probieren).
///
/// Alle gefundenen Label-Serien des ersten passenden Namens werden summiert
/// (parse_gauge-Verhalten); Multi-Rank-Ueberzaehlung ist eine dokumentierte
/// Limitation (kein Clampen).
pub fn kv_cache_usage(body: &str) -> Option<f64> {
    const KV_CACHE_NAMES: &[&str] = &[
        "sglang:token_usage",
        "vllm:kv_cache_usage_perc",
    ];
    KV_CACHE_NAMES.iter().find_map(|n| parse_gauge(body, n))
}

/// Heuristik: sieht der body aus wie Prometheus-Textformat?
///
/// Zhaelt Daten-Zeilen (name{labels} value) und prueft, ob mindestens zwei
/// solche Zeilen existieren oder eine Daten-Zeile + eine # HELP/# TYPE-Header-Zeile.
/// Damit werden HTML-/JSON-Fehlerseiten (404, CORS, ...) aussortiert.
pub fn looks_like_prometheus(body: &str) -> bool {
    let mut data_lines = 0;
    let mut has_header = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            if line.starts_with("# HELP") || line.starts_with("# TYPE") {
                has_header = true;
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let Some((key_part, value_part)) = line.split_once(' ') else {
            continue;
        };
        let name = key_part.split('{').next().unwrap_or(key_part);
        let name_ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':');
        let value_ok = value_part
            .split_whitespace()
            .next()
            .is_some_and(|t| t.parse::<f64>().is_ok());
        if name_ok && value_ok {
            data_lines += 1;
        }
    }
    data_lines >= 2 || (data_lines == 1 && has_header)
}

/// Decode-geschwindigkeit (token/s) aus dem provider-metrics-body lesen.
/// vLLM meldet `time_per_output_token_seconds` als histogramm; aus
/// `_sum`/`_count` ergibt sich die durchschnittliche sekunden-anzahl pro
/// token, invertiert = token/s. Mehrere gelabelte serien werden summiert
/// (Sigma sum / Sigma count), unlabeled-aggregaten werden nicht doppelt
/// gezahlt (siehe parse_gauge). None, falls das metric nicht vorhanden ist.
pub fn decode_tps(body: &str) -> Option<f64> {
    const NAMES: &[&str] = &[
        "vllm:time_per_output_token_seconds",
        "time_per_output_token_seconds",
    ];
    NAMES.iter().find_map(|n| {
        let sum = parse_gauge(body, &format!("{n}_sum"))?;
        let count = parse_gauge(body, &format!("{n}_count"))?;
        if count > 0.0 && sum > 0.0 {
            Some(count / sum)
        } else {
            None
        }
    })
}

/// Parse generation/prompt token counters aus dem metrics-body.
///
/// Erwartet monoton steigende Counter (Prometheus-Counter-Semantik).
/// Labeled Serien werden summiert; unlabeled-Zeilen werden nur als Fallback
/// verwertet, wenn keine gelabelte Serie existiert (analog `parse_gauge`).
///
/// Gibt `(generation_tokens_total, prompt_tokens_total)` zurueck.
pub fn parse_token_counters(body: &str) -> (Option<f64>, Option<f64>) {
    let gen = parse_gauge(body, "vllm:generation_tokens_total")
        .or_else(|| parse_gauge(body, "sglang:generation_tokens_total"))
        .or_else(|| parse_gauge(body, "generation_tokens_total"));
    let prompt = parse_gauge(body, "vllm:prompt_tokens_total")
        .or_else(|| parse_gauge(body, "sglang:prompt_tokens_total"))
        .or_else(|| parse_gauge(body, "prompt_tokens_total"));
    (gen, prompt)
}

/// Reine Funktion zur Berechnung der Rate aus zwei Counter-Samples.
///
/// - `prev`: Optionales vorheriges Sample `(Zeitpunkt, Wert)`. `None` => None (erster Sample).
/// - `current`: aktueller Counter-Wert.
/// - `now`: Zeitpunkt des aktuellen Samples.
///
/// Gibt `Some(rate)` zurueck, wenn:
///   - ein vorheriges Sample existiert,
///   - mindestens `MIN_ELAPSED_SECS` Sekunden vergangen sind,
///   - der Counter nicht zurueckgesetzt wurde (cur >= prev).
///
/// Bei Reset (cur < prev) wird `None` zurueckgegeben (Baseline muss neu gesetzt werden).
pub const MIN_ELAPSED_SECS: f64 = 1.0;

pub fn counter_delta_rate(
    prev: Option<(&Instant, f64)>,
    current: f64,
    now: Instant,
) -> Option<f64> {
    let (prev_time, prev_val) = prev?;
    let elapsed = now.duration_since(*prev_time).as_secs_f64();
    if elapsed < MIN_ELAPSED_SECS {
        return None;
    }
    if current < prev_val {
        // Counter-Reset erkannt
        return None;
    }
    Some((current - prev_val) / elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# HELP vllm:num_requests_waiting Number of requests waiting in queue.
# TYPE vllm:num_requests_waiting gauge
vllm:num_requests_waiting{model_name="qwen3.8-27b"} 3
# HELP vllm:num_requests_running Number of requests currently running.
# TYPE vllm:num_requests_running gauge
vllm:num_requests_running{model_name="qwen3.8-27b"} 8
vllm:some_other_metric 42.5
"#;

    const SGLANG_SAMPLE: &str = r#"
# TYPE sglang:num_running_reqs gauge
sglang:num_running_reqs{engine_type="unified",model_name="Qwen/Qwen3.8-27B-FP8"} 3
# TYPE sglang:num_queue_reqs gauge
sglang:num_queue_reqs{engine_type="unified",model_name="Qwen/Qwen3.8-27B-FP8"} 1
"#;

    #[test]
    fn test_parse_vllm_metrics() {
        let (queued, running) = queue_metrics(SAMPLE);
        assert_eq!(queued, Some(3.0));
        assert_eq!(running, Some(8.0));
    }

    #[test]
    fn test_parse_tgi_metrics() {
        let body = "# HELP tgi:queue_size gauge\ntgi:queue_size 5\ntgi:num_requests_running 2\n";
        let (queued, running) = queue_metrics(body);
        assert_eq!(queued, Some(5.0));
        assert_eq!(running, Some(2.0));
    }

    #[test]
    fn test_parse_sglang_metrics() {
        let (queued, running) = queue_metrics(SGLANG_SAMPLE);
        assert_eq!(queued, Some(1.0));
        assert_eq!(running, Some(3.0));
    }

    #[test]
    fn test_parse_sglang_queue_only() {
        let (queued, running) = queue_metrics("sglang:num_queue_reqs 2\n");
        assert_eq!(queued, Some(2.0));
        assert_eq!(running, None);
    }

    #[test]
    fn test_no_metrics_found() {
        let (queued, running) = queue_metrics("up 1\n");
        assert_eq!(queued, None);
        assert_eq!(running, None);
    }

    #[test]
    fn test_looks_like_prometheus() {
        assert!(looks_like_prometheus(SAMPLE));
        assert!(looks_like_prometheus(
            "# HELP queue_size Gauge\n# TYPE queue_size gauge\nqueue_size 5\n"
        ));
    }

    #[test]
    fn test_looks_like_prometheus_rejects_non_metrics() {
        // HTML-Fehlerseite
        assert!(!looks_like_prometheus("<html><body>404 Not Found</body></html>"));
        // JSON (z.B. OpenAI-Error-Body)
        assert!(!looks_like_prometheus(r#"{"error":{"message":"not found"}}"#));
        // einzelne Daten-Zeile ohne Header
        assert!(!looks_like_prometheus("up 1\n"));
        // leer
        assert!(!looks_like_prometheus(""));
        assert!(!looks_like_prometheus("# HELP x gauge\n"));
    }

    #[test]
    fn test_decode_tps_from_vllm_histogram() {
        let body = "# TYPE vllm:time_per_output_token_seconds histogram\n\
                    vllm:time_per_output_token_seconds_bucket{le=\"0.01\"} 0\n\
                    vllm:time_per_output_token_seconds_sum 2.0\n\
                    vllm:time_per_output_token_seconds_count 100.0\n";
        // 100 tokens / 2.0 s = 50 token/s
        assert!((decode_tps(body).unwrap() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_decode_tps_missing_returns_none() {
        assert_eq!(decode_tps(SAMPLE), None);
    }

    #[test]
    fn test_decode_tps_zero_count_returns_none() {
        let body = "vllm:time_per_output_token_seconds_sum 0\nvllm:time_per_output_token_seconds_count 0\n";
        assert_eq!(decode_tps(body), None);
    }

    #[test]
    fn test_ignores_comments_and_sums_repeated_lines() {
        let body = "# TYPE num_requests_waiting gauge\nnum_requests_waiting 1\nnum_requests_waiting 7\n";
        assert_eq!(parse_gauge(body, "num_requests_waiting"), Some(8.0));
    }

    #[test]
    fn test_sums_multiple_unlabeled_lines() {
        // Akkulations-Verhalten: mehrere unlabeled-Zeilen desselben metric-namens
        // werden separat summiert (kein overwrite).
        let body = "queue_size 3\nqueue_size 4\n";
        assert_eq!(parse_gauge(body, "queue_size"), Some(7.0));
    }

    #[test]
    fn test_only_unlabeled_nan_inf_returns_none() {
        // Prometheus-Body mit NUR unlabeled-Zeilen, deren Werte NaN/+Inf/-Inf sind:
        // alle werden uebersprungen -> None, kein NaN in der Summe, kein Panic.
        let body = "num_requests_waiting NaN\nnum_requests_running +Inf\nqueue_size -Inf\n";
        assert_eq!(parse_gauge(body, "num_requests_waiting"), None);
        assert_eq!(parse_gauge(body, "num_requests_running"), None);
        assert_eq!(parse_gauge(body, "queue_size"), None);
        let (queued, running) = queue_metrics(body);
        assert_eq!(queued, None);
        assert_eq!(running, None);
    }

    #[test]
    fn test_sums_multiple_labeled_series() {
        let body = "vllm:num_requests_running{model_name=\"a\"} 3\nvllm:num_requests_running{model_name=\"b\"} 5\n";
        assert_eq!(parse_gauge(body, "vllm:num_requests_running"), Some(8.0));
    }

    #[test]
    fn test_ignores_non_finite_values() {
        let body = "num_requests_running{a=\"1\"} NaN\nnum_requests_running{a=\"2\"} +Inf\nnum_requests_running{a=\"3\"} 4\n";
        assert_eq!(parse_gauge(body, "num_requests_running"), Some(4.0));
        // nur nicht-endliche werte -> keine serie
        assert_eq!(parse_gauge("num_requests_running{a} NaN\n", "num_requests_running"), None);
    }

    #[test]
    fn test_unlabeled_not_double_counted_when_labeled_present() {
        let body = "num_requests_waiting 99\nnum_requests_waiting{model_name=\"a\"} 2\nnum_requests_waiting{model_name=\"b\"} 3\n";
        // nur die gelabelten serien (2+3), unlabeled-aggregat (99) wird ignoriert
        assert_eq!(parse_gauge(body, "num_requests_waiting"), Some(5.0));
    }

    #[test]
    fn test_parse_token_counters_labeled() {
        let body = "vllm:generation_tokens_total{model_name=\"a\"} 100\n\
                    vllm:generation_tokens_total{model_name=\"b\"} 200\n\
                    vllm:prompt_tokens_total{model_name=\"a\"} 50\n\
                    vllm:prompt_tokens_total{model_name=\"b\"} 75\n";
        let (gen, prompt) = parse_token_counters(body);
        assert_eq!(gen, Some(300.0));
        assert_eq!(prompt, Some(125.0));
    }

    #[test]
    fn test_parse_token_counters_unlabeled_fallback() {
        let body = "generation_tokens_total 42\nprompt_tokens_total 17\n";
        let (gen, prompt) = parse_token_counters(body);
        assert_eq!(gen, Some(42.0));
        assert_eq!(prompt, Some(17.0));
    }

    #[test]
    fn test_parse_sglang_token_counters() {
        let body = "sglang:generation_tokens_total{model_name=\"a\"} 12345\n\
                    sglang:prompt_tokens_total{model_name=\"a\"} 678\n";
        let (gen, prompt) = parse_token_counters(body);
        assert_eq!(gen, Some(12345.0));
        assert_eq!(prompt, Some(678.0));
    }

    #[test]
    fn test_parse_token_counters_missing() {
        let body = "up 1\n";
        let (gen, prompt) = parse_token_counters(body);
        assert_eq!(gen, None);
        assert_eq!(prompt, None);
    }

    #[test]
    fn test_parse_token_counters_labeled_preferred_over_unlabeled() {
        let body = "vllm:generation_tokens_total 999\n\
                    vllm:generation_tokens_total{model_name=\"a\"} 10\n\
                    vllm:prompt_tokens_total 888\n\
                    vllm:prompt_tokens_total{model_name=\"a\"} 20\n";
        let (gen, prompt) = parse_token_counters(body);
        assert_eq!(gen, Some(10.0));
        assert_eq!(prompt, Some(20.0));
    }

    #[test]
    fn test_kv_cache_usage_vllm() {
        let body = "vllm:kv_cache_usage_perc{model_name=\"qwen\"} 0.42\n";
        assert!((kv_cache_usage(body).unwrap() - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn test_kv_cache_usage_sglang_token_usage() {
        // diese SGLang-Version exportiert nur token_usage als Aequivalent
        let body = "sglang:token_usage{model_name=\"qwen\"} 0.75\n";
        assert!((kv_cache_usage(body).unwrap() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_kv_cache_usage_tgi_body_returns_none() {
        // TGI exportiert in aktuellen Versionen keine kv-cache-metrik, daher
        // liefert ein realistisches TGI-Body ohne cache-metrik None.
        let body = "tgi:queue_size 5\ntgi:num_requests_running 2\n";
        assert_eq!(kv_cache_usage(body), None);
    }

    #[test]
    fn test_kv_cache_usage_missing_returns_none() {
        let body = "up 1\nvllm:num_requests_waiting 0\n";
        assert_eq!(kv_cache_usage(body), None);
    }

    #[test]
    fn test_kv_cache_usage_nan_returns_none() {
        // parse_gauge faengt nicht-endliche werte ab -> None statt NaN
        let body = "vllm:kv_cache_usage_perc{model_name=\"a\"} NaN\n";
        assert_eq!(kv_cache_usage(body), None);
    }

    #[test]
    fn test_kv_cache_usage_first_name_wins() {
        // beides vorhanden: erster Name der Liste (sglang:token_usage) gewinnt
        let body = "sglang:token_usage 0.5\nvllm:kv_cache_usage_perc 0.9\n";
        assert!((kv_cache_usage(body).unwrap() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_kv_cache_usage_sums_labeled_series() {
        let body = "sglang:token_usage{a=\"1\"} 0.2\nsglang:token_usage{a=\"2\"} 0.3\n";
        assert!((kv_cache_usage(body).unwrap() - 0.5).abs() < f64::EPSILON);
    }

    // --- counter_delta_rate tests ---

    #[test]
    fn test_counter_delta_rate_first_sample_returns_none() {
        let now = Instant::now();
        assert_eq!(counter_delta_rate(None, 100.0, now), None);
    }

    #[test]
    fn test_counter_delta_rate_normal() {
        let prev_time = Instant::now();
        let now = prev_time + std::time::Duration::from_secs(10);
        let rate = counter_delta_rate(Some((&prev_time, 50.0)), 150.0, now);
        assert_eq!(rate, Some(10.0)); // (150-50)/10 = 10
    }

    #[test]
    fn test_counter_delta_rate_reset_returns_none() {
        let prev_time = Instant::now();
        let now = prev_time + std::time::Duration::from_secs(10);
        // current < prev => reset
        assert_eq!(counter_delta_rate(Some((&prev_time, 500.0)), 10.0, now), None);
    }

    #[test]
    fn test_counter_delta_rate_elapsed_guard() {
        let prev_time = Instant::now();
        let now = prev_time + std::time::Duration::from_millis(500); // < 1s
        assert_eq!(counter_delta_rate(Some((&prev_time, 50.0)), 100.0, now), None);
    }

    #[test]
    fn test_counter_delta_rate_same_value() {
        let prev_time = Instant::now();
        let now = prev_time + std::time::Duration::from_secs(5);
        assert_eq!(counter_delta_rate(Some((&prev_time, 100.0)), 100.0, now), Some(0.0));
    }

    #[test]
    fn test_decode_tps_sums_labeled_histograms() {
        let body = "vllm:time_per_output_token_seconds_sum{model_name=\"a\"} 2.0\n\
                    vllm:time_per_output_token_seconds_sum{model_name=\"b\"} 3.0\n\
                    vllm:time_per_output_token_seconds_count{model_name=\"a\"} 100.0\n\
                    vllm:time_per_output_token_seconds_count{model_name=\"b\"} 50.0\n";
        // (100+50) tokens / (2+3) s = 30 token/s
        assert!((decode_tps(body).unwrap() - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_decode_tps_ignores_unlabeled_aggregate() {
        let body = "vllm:time_per_output_token_seconds_sum 999.0\n\
                    vllm:time_per_output_token_seconds_count 999.0\n\
                    vllm:time_per_output_token_seconds_sum{model_name=\"a\"} 2.0\n\
                    vllm:time_per_output_token_seconds_count{model_name=\"a\"} 100.0\n";
        // gelabelte serie zahlt: 100 / 2 = 50 token/s (nicht ueber 999)
        assert!((decode_tps(body).unwrap() - 50.0).abs() < f64::EPSILON);
    }
}
