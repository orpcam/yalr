//! Async Log-Ingest: Request-Logs werden ueber einen Channel an einen
//! Hintergrund-Task geschickt, der sie gebatcht nach ClickHouse schreibt.
//! Der Proxy-Hot-Path blockiert dadurch nie auf die Datenbank.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

/// Ein vollstaendiger Request-Log-Eintrag.
#[derive(Debug, Clone, Serialize, clickhouse::Row)]
pub struct RequestLog {
    #[serde(with = "clickhouse::serde::uuid")]
    pub id: Uuid,
    pub request_id: String,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub timestamp: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub virtual_key_id: Option<Uuid>,
    pub key_name: String,
    pub provider: String,
    pub provider_name: String,
    /// Stable ID des providers: erlaubt dem dashboard, historische logs
    /// nach einem rename dem aktuellen namen zuzuordnen.
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub provider_id: Option<Uuid>,
    pub model: String,
    pub upstream_model: String,
    pub endpoint: String,
    pub status: u16,
    pub error_message: String,
    pub error_type: String,
    pub is_stream: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub first_byte_ms: u64,
    pub request_body: String,
    pub response_body: String,
    pub request_truncated: bool,
    pub response_truncated: bool,
    /// Bedient durch Fallback: der bedienende Target hat ein anderes Modell
    /// geliefert, als der Client angefragt hatte.
    pub is_fallback: bool,
    /// Vom Client angefordertes Modell (vor Routing/Fallback).
    pub original_model: String,
    /// Anzahl der Target-Versuche bis zum bedienenden Target (0 = unbekannt,
    /// z.B. Alt-Daten oder Stream-Pfade ohne Attempt-Kontext).
    pub attempts_made: u8,
}

/// Reduzierter Log-Eintrag fuer Live-Events (ohne Bodies, um Payload klein zu halten).
#[derive(Debug, Clone, Serialize)]
pub struct LiveLog {
    pub id: Uuid,
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    pub key_name: String,
    pub provider: String,
    pub provider_name: String,
    pub model: String,
    pub upstream_model: String,
    pub endpoint: String,
    pub status: u16,
    pub error_type: String,
    pub is_stream: bool,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
    pub first_byte_ms: u64,
    pub is_fallback: bool,
    pub original_model: String,
    pub attempts_made: u8,
}

impl From<&RequestLog> for LiveLog {
    fn from(log: &RequestLog) -> Self {
        Self {
            id: log.id,
            request_id: log.request_id.clone(),
            timestamp: log.timestamp,
            key_name: log.key_name.clone(),
            provider: log.provider.clone(),
            provider_name: log.provider_name.clone(),
            model: log.model.clone(),
            upstream_model: log.upstream_model.clone(),
            endpoint: log.endpoint.clone(),
            status: log.status,
            error_type: log.error_type.clone(),
            is_stream: log.is_stream,
            prompt_tokens: log.prompt_tokens,
            completion_tokens: log.completion_tokens,
            cost_usd: log.cost_usd,
            duration_ms: log.duration_ms,
            first_byte_ms: log.first_byte_ms,
            is_fallback: log.is_fallback,
            original_model: log.original_model.clone(),
            attempts_made: log.attempts_made,
        }
    }
}

/// Live-Event fuer das Dashboard (SSE): request lifecycle.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveEvent {
    RequestStarted {
        request_id: String,
        timestamp: DateTime<Utc>,
        key_name: String,
        provider: String,
        provider_name: String,
        model: String,
        endpoint: String,
        is_stream: bool,
    },
    FirstByte {
        request_id: String,
        first_byte_ms: u64,
    },
    Completed {
        log: LiveLog,
    },
}

/// Kapazitaet des Overflow-Puffers, falls der mpsc-Channel voll ist.
const OVERFLOW_CAP: usize = 20_000;

/// TTL fuer in-flight Registry-Eintraege (15 Minuten).
const IN_FLIGHT_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Ein in-flight Request, der noch nicht abgeschlossen ist.
#[derive(Debug, Clone, Serialize)]
pub struct InFlightReq {
    pub request_id: String,
    pub provider: String,
    pub provider_name: String,
    pub model: String,
    pub key_name: String,
    pub started_at_ms: u64,
    pub first_byte_ms: Option<u64>,
}

/// Zähler-Zustand pro (provider, provider_name, model) seit Prozessstart.
#[derive(Debug, Clone, Default)]
struct Count {
    requests: u64,
    errors: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    cost_usd_micros: u64,
}

/// Öffentliche Ansicht eines (provider, provider_name, model)-Zählers für das
/// Dashboard.
#[derive(Debug, Clone, Default)]
pub struct CounterEntry {
    pub provider: String,
    pub provider_name: String,
    pub model: String,
    pub requests: u64,
    pub errors: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_usd_micros: u64,
}

/// Handle zum Einspeisen von Logs (billig zu klonen, fuer jeden Request).
#[derive(Clone)]
pub struct LogSink {
    tx: mpsc::Sender<RequestLog>,
    live_tx: broadcast::Sender<LiveEvent>,
    overflow: Arc<Mutex<VecDeque<RequestLog>>>,
    dropped: Arc<AtomicU64>,
    counters: Arc<Mutex<HashMap<(String, String, String), Count>>>,
    auth_failures: Arc<AtomicU64>,
    in_flight: Arc<Mutex<HashMap<String, InFlightReq>>>,
}

impl LogSink {
    /// Log einreichen - blockiert nie (bounded channel, Fallback auf
    /// begrenzten Overflow-Puffer). Zusätzlich wird ein
    /// Completed-Live-Event an das Dashboard gebroadcastet.
    pub fn log(&self, entry: RequestLog) {
        self.record(&entry);
        // emit() (nicht direktes live_tx.send), damit der in-flight-Registry-
        // Eintrag beim Completed-Event entfernt wird (sonst Geister-Eintraege
        // bis zur 15-Min-TTL bei Non-Streaming-Requests und Fehlerpfaden).
        self.emit(LiveEvent::Completed {
            log: LiveLog::from(&entry),
        });
        self.try_push(entry);
    }

    /// Nur persistieren (Channel), ohne Completed-Event. Fuer streaming-pfade,
    /// die das Completed-Event frueher (am stream-end) selbst senden.
    pub fn log_only(&self, entry: RequestLog) {
        self.record(&entry);
        self.try_push(entry);
    }

    /// Zentraler Zähler-Update: wird fuer jeden abgeschlossenen Request
    /// aufgerufen (Non-Stream ueber `log`, Stream ueber `log_only`).
    ///
    /// Definition:
    /// - `requests`: alle geloggten Zeilen mit status != 401/402
    /// - `errors`: davon status >= 400
    /// - `401` (ungültiger Key) und `402` (Budget überschritten,
    ///   common::auth): nicht in `requests`/`errors`, sondern in
    ///   `auth_failures` — beide sind Client-Rektionen vor dem Proxying
    /// - Einträge mit leerem `provider` (gateway-interne Fehler wie
    ///   400/404/502 aus `error_response`) landen unter dem
    ///   Label-Fallback `provider = "gateway"`, nicht in einem `""`-Bucket.
    ///     Das provider_name-Label bleibt dabei unverändert (dort ""), es
    ///     gibt keinen zweiten Fallback.
    /// - `cost_usd` wird als Mikro-USD (u64) gehalten, um f64-Atomicitaet zu vermeiden.
    fn record(&self, entry: &RequestLog) {
        let cost_micros = (entry.cost_usd * 1e6).round() as u64;
        let is_auth = entry.status == 401 || entry.status == 402;
        if is_auth {
            self.auth_failures.fetch_add(1, Ordering::Relaxed);
        } else {
            let provider = if entry.provider.is_empty() {
                "gateway"
            } else {
                entry.provider.as_str()
            };
            let mut map = self.counters.lock().unwrap();
            // Key: (provider-kind, instanzname, model); provider_name ohne
            // Fallback (gateway-bucket: "").
            let c = map
                .entry((
                    provider.to_string(),
                    entry.provider_name.clone(),
                    entry.model.clone(),
                ))
                .or_default();
            c.requests += 1;
            c.prompt_tokens += entry.prompt_tokens;
            c.completion_tokens += entry.completion_tokens;
            c.cost_usd_micros += cost_micros;
            if entry.status >= 400 {
                c.errors += 1;
            }
        }
    }

    /// Zählt einen Auth-Fehler, der nie in die Request-Logs gelangt
    /// (z. B. 401 auf /v1/models): inkrementiert NUR den
    /// auth_failures-Counter — kein ClickHouse-Log, keine anderen Zähler.
    pub fn record_auth_failure(&self) {
        self.auth_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Aktuelle Zähler-Zustände (since process start) + Auth-Failures lesen.
    pub fn snapshot_counters(&self) -> (Vec<CounterEntry>, u64) {
        let map = self.counters.lock().unwrap();
        let entries = map
            .iter()
            .map(|((provider, provider_name, model), c)| CounterEntry {
                provider: provider.clone(),
                provider_name: provider_name.clone(),
                model: model.clone(),
                requests: c.requests,
                errors: c.errors,
                prompt_tokens: c.prompt_tokens,
                completion_tokens: c.completion_tokens,
                cost_usd_micros: c.cost_usd_micros,
            })
            .collect();
        (entries, self.auth_failures.load(Ordering::Relaxed))
    }

    /// Anzahl an wegen Overflow/Flush-Fehlern verworfenen Log-Einträgen.
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn try_push(&self, entry: RequestLog) {
        match self.tx.try_send(entry) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(entry)) => {
                push_overflow(&self.overflow, &self.dropped, OVERFLOW_CAP, entry);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("ingest channel closed - log entry dropped");
            }
        }
    }

    /// Live-Event senden (Best-Effort, blockiert nie).
    /// Waehlt dabei die in-flight Registry auf.
    pub fn emit(&self, event: LiveEvent) {
        match &event {
            LiveEvent::RequestStarted {
                request_id,
                timestamp,
                key_name,
                provider,
                provider_name,
                model,
                ..
            } => {
                let mut guard = self.in_flight.lock().unwrap();
                guard.insert(
                    request_id.clone(),
                    InFlightReq {
                        request_id: request_id.clone(),
                        provider: provider.clone(),
                        provider_name: provider_name.clone(),
                        model: model.clone(),
                        key_name: key_name.clone(),
                        started_at_ms: timestamp.timestamp_millis() as u64,
                        first_byte_ms: None,
                    },
                );
            }
            LiveEvent::FirstByte { request_id, first_byte_ms } => {
                let mut guard = self.in_flight.lock().unwrap();
                if let Some(entry) = guard.get_mut(request_id) {
                    entry.first_byte_ms = Some(*first_byte_ms);
                }
            }
            LiveEvent::Completed { log } => {
                let mut guard = self.in_flight.lock().unwrap();
                guard.remove(&log.request_id);
            }
        }
        let _ = self.live_tx.send(event);
    }

    /// Liefert eine Snapshot-Liste aller in-flight Requests, sortiert nach
    /// `started_at_ms` aufsteigend. Eintraege aelter als 15 Minuten werden
    /// dabei verworfen (TTL-Sweep).
    pub fn in_flight_snapshot(&self) -> Vec<InFlightReq> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let ttl_ms = IN_FLIGHT_TTL.as_millis() as u64;

        let mut guard = self.in_flight.lock().unwrap();
        guard.retain(|_, v| now_ms.saturating_sub(v.started_at_ms) < ttl_ms);

        let mut entries: Vec<InFlightReq> = guard.values().cloned().collect();
        entries.sort_by_key(|e| e.started_at_ms);
        entries
    }

    /// Empfänger für Live-Events (für das Dashboard-SSE).
    pub fn subscribe(&self) -> broadcast::Receiver<LiveEvent> {
        self.live_tx.subscribe()
    }
}

/// Startet den Hintergrund-Writer.
///
/// - `buffer_size`: max. Eintraege pro Batch-Insert
/// - `flush_interval`: max. Zeit, die Eintraege im Buffer verbringen
/// - `body_max_len`: max. Laenge von request/response body im Log (Rest wird abgeschnitten)
pub struct IngestConfig {
    pub buffer_size: usize,
    pub flush_interval: Duration,
    pub body_max_len: usize,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            buffer_size: 500,
            flush_interval: Duration::from_secs(2),
            body_max_len: 32 * 1024,
        }
    }
}

/// Erzeugt Sink + startet den Hintergrund-Task.
/// Gibt einen JoinHandle zurueck, der beim Shutdown gedroppt werden kann.
pub fn start(
    client: clickhouse::Client,
    config: IngestConfig,
) -> (LogSink, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<RequestLog>(10_000);
    let (live_tx, _) = broadcast::channel::<LiveEvent>(256);
    let overflow = Arc::new(Mutex::new(VecDeque::with_capacity(OVERFLOW_CAP)));
    let dropped = Arc::new(AtomicU64::new(0));
    let sink = LogSink {
        tx,
        live_tx,
        overflow: overflow.clone(),
        dropped: dropped.clone(),
        counters: Arc::new(Mutex::new(HashMap::new())),
        auth_failures: Arc::new(AtomicU64::new(0)),
        in_flight: Arc::new(Mutex::new(HashMap::new())),
    };

    let handle = tokio::spawn(async move {
        let mut buffer: Vec<RequestLog> = Vec::with_capacity(config.buffer_size);
        let mut interval = tokio::time::interval(config.flush_interval);

        loop {
            tokio::select! {
                maybe_entry = rx.recv() => {
                    match maybe_entry {
                        Some(entry) => {
                            buffer.push(truncate_bodies(entry, config.body_max_len));
                            if buffer.len() >= config.buffer_size {
                                drain_overflow(&overflow, &mut buffer, config.body_max_len);
                                flush(&client, &mut buffer, &dropped).await;
                            }
                        }
                        None => {
                            // channel geschlossen: Overflow leeren, letzten Flush
                            // machen und beenden
                            drain_overflow(&overflow, &mut buffer, config.body_max_len);
                            flush(&client, &mut buffer, &dropped).await;
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    drain_overflow(&overflow, &mut buffer, config.body_max_len);
                    if !buffer.is_empty() {
                        flush(&client, &mut buffer, &dropped).await;
                    }
                }
            }
        }
    });

    (sink, handle)
}

/// Char-boundary-sicheres Truncaten (String::truncate panikt, wenn die
/// Ziel-Laenge mitten in einem Multi-Byte-UTF-8-Char liegt).
fn truncate_str(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

fn truncate_bodies(mut entry: RequestLog, max_len: usize) -> RequestLog {
    if entry.request_body.len() > max_len {
        truncate_str(&mut entry.request_body, max_len);
        entry.request_truncated = true;
    }
    if entry.response_body.len() > max_len {
        truncate_str(&mut entry.response_body, max_len);
        entry.response_truncated = true;
    }
    entry
}

/// Schickt einen Eintrag in den Overflow-Puffer. Ist der Puffer voll, wird
/// der aelteste Eintrag droppgt, der Drop-Zaehler hochgezuehlt und ein
/// Error-Log geschrieben.
fn push_overflow(
    overflow: &Mutex<VecDeque<RequestLog>>,
    dropped: &AtomicU64,
    cap: usize,
    entry: RequestLog,
) {
    let mut q = overflow.lock().unwrap();
    if q.len() >= cap {
        q.pop_front();
        let total = dropped.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::error!(
            "ingest overflow full (cap {cap}) - oldest entry dropped (total dropped: {total})"
        );
    }
    q.push_back(entry);
}

/// Leert den Overflow-Puffer in den Buffer (mit Body-Truncation).
fn drain_overflow(
    overflow: &Mutex<VecDeque<RequestLog>>,
    buffer: &mut Vec<RequestLog>,
    body_max_len: usize,
) {
    let mut q = overflow.lock().unwrap();
    while let Some(entry) = q.pop_front() {
        buffer.push(truncate_bodies(entry, body_max_len));
    }
}

/// Schreibt einen Batch nach ClickHouse. Bei Fehlern wird der Batch behalten
/// und mit einfachem Backoff wiederholt (max. 3 Versuche); erst danach wird
/// er droppgt und im Drop-Zaehler verzeichnet. Duplikate sind nur im seltenen
/// Fall eines verlorenen Responses nach serverseitigem Commit moglich;
/// akzeptiert (MergeTree dedupliziert nicht).
async fn flush(
    client: &clickhouse::Client,
    buffer: &mut Vec<RequestLog>,
    dropped: &AtomicU64,
) {
    if buffer.is_empty() {
        return;
    }
    let entries = std::mem::take(buffer);
    const MAX_ATTEMPTS: u32 = 3;

    for attempt in 1..MAX_ATTEMPTS {
        match try_insert(client, &entries).await {
            Ok(()) => {
                tracing::debug!("flushed {} log entries to clickhouse", entries.len());
                return;
            }
            Err(e) => {
                let backoff = Duration::from_millis(500u64 * 2u64.pow(attempt - 1));
                tracing::warn!(
                    "clickhouse flush failed (attempt {attempt}/{MAX_ATTEMPTS}): {e} - retrying in {backoff:?}"
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }

    // letzter Versuch: danach Batch droppen und verzeichnen
    match try_insert(client, &entries).await {
        Ok(()) => {
            tracing::debug!("flushed {} log entries to clickhouse", entries.len());
        }
        Err(e) => {
            let lost = entries.len();
            let total = dropped.fetch_add(lost as u64, Ordering::Relaxed) + lost as u64;
            tracing::error!(
                "clickhouse flush failed after {MAX_ATTEMPTS} attempts: {e} ({lost} entries lost, total dropped: {total})"
            );
        }
    }
}

/// Ein kompletter Insert-Versuch fuer einen Batch (ein Batch = ein einzelnes Insert).
async fn try_insert(client: &clickhouse::Client, entries: &[RequestLog]) -> Result<(), String> {
    let mut insert = client
        .insert("yalr.request_logs")
        .map_err(|e| e.to_string())?;
    for entry in entries {
        insert.write(entry).await.map_err(|e| e.to_string())?;
    }
    insert.end().await.map_err(|e| e.to_string())
}

/// Legt das ClickHouse-Schema an (idempotent), falls die Tabelle noch nicht existiert.
///
/// Wichtig: `client` darf hier KEINE `with_database(...)` gesetzt haben, sonst
/// lehnt ClickHouse bereits `CREATE DATABASE` mit UNKNOWN_DATABASE ab, wenn die
/// DB noch nicht existiert (der Client sendet `?database=...` bei jedem Query).
pub async fn ensure_schema(client: &clickhouse::Client) -> anyhow::Result<()> {
    client
        .query("CREATE DATABASE IF NOT EXISTS yalr")
        .execute()
        .await?;
    client
        .query(
            r#"
            CREATE TABLE IF NOT EXISTS yalr.request_logs
            (
                id            UUID,
                request_id    String,
                timestamp     DateTime64(3, 'UTC'),
                virtual_key_id Nullable(UUID),
                key_name      LowCardinality(String),
                provider      LowCardinality(String),
                provider_name LowCardinality(String),
                model         LowCardinality(String),
                upstream_model LowCardinality(String),
                endpoint      LowCardinality(String),
                status        UInt16,
                error_message String DEFAULT '',
                error_type    LowCardinality(String) DEFAULT '',
                is_stream     Bool,
                prompt_tokens UInt64 DEFAULT 0,
                completion_tokens UInt64 DEFAULT 0,
                total_tokens  UInt64 DEFAULT 0,
                cost_usd      Float64 DEFAULT 0,
                duration_ms   UInt64 DEFAULT 0,
                first_byte_ms UInt64 DEFAULT 0,
                request_body  String DEFAULT '',
                response_body String DEFAULT '',
                request_truncated  Bool DEFAULT false,
                response_truncated Bool DEFAULT false,
                is_fallback       Bool DEFAULT false,
                original_model    LowCardinality(String) DEFAULT '',
                attempts_made     UInt8 DEFAULT 0,
                provider_id   Nullable(UUID)
            )
            ENGINE = MergeTree
            PARTITION BY toYYYYMM(timestamp)
            ORDER BY (timestamp, key_name, provider, model)
            TTL toDateTime(timestamp) + INTERVAL 90 DAY
            SETTINGS index_granularity = 8192
            "#,
        )
        .execute()
        .await?;

    // Bestandstabellen: spalten idempotent nachziehen (CREATE IF NOT EXISTS
    // greift bei existierender Tabelle nicht)
    client
        .query("ALTER TABLE yalr.request_logs ADD COLUMN IF NOT EXISTS request_truncated Bool DEFAULT false")
        .execute()
        .await?;
    client
        .query("ALTER TABLE yalr.request_logs ADD COLUMN IF NOT EXISTS response_truncated Bool DEFAULT false")
        .execute()
        .await?;
    client
        .query("ALTER TABLE yalr.request_logs ADD COLUMN IF NOT EXISTS provider_id Nullable(UUID)")
        .execute()
        .await?;
    client
        .query("ALTER TABLE yalr.request_logs ADD COLUMN IF NOT EXISTS is_fallback Bool DEFAULT false")
        .execute()
        .await?;
    client
        .query("ALTER TABLE yalr.request_logs ADD COLUMN IF NOT EXISTS original_model LowCardinality(String) DEFAULT ''")
        .execute()
        .await?;
    client
        .query("ALTER TABLE yalr.request_logs ADD COLUMN IF NOT EXISTS attempts_made UInt8 DEFAULT 0")
        .execute()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sink_channel() {
        let (tx, mut rx) = mpsc::channel::<RequestLog>(10);
        let (live_tx, _live_rx) = broadcast::channel::<LiveEvent>(16);
        let sink = LogSink {
            tx,
            live_tx,
            overflow: Arc::new(Mutex::new(VecDeque::new())),
            dropped: Arc::new(AtomicU64::new(0)),
            counters: Arc::new(Mutex::new(HashMap::new())),
            auth_failures: Arc::new(AtomicU64::new(0)),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        };
        let entry = RequestLog {
            id: Uuid::new_v4(),
            request_id: "req_test".into(),
            timestamp: Utc::now(),
            virtual_key_id: None,
            key_name: "test".into(),
            provider: "openai".into(),
            provider_name: "openai-main".into(),
            provider_id: None,
            model: "gpt-4o".into(),
            upstream_model: "gpt-4o".into(),
            endpoint: "/v1/chat/completions".into(),
            status: 200,
            error_message: String::new(),
            error_type: String::new(),
            is_stream: false,
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            cost_usd: 0.001,
            duration_ms: 100,
            first_byte_ms: 50,
            request_body: "{}".into(),
            response_body: "{}".into(),
            request_truncated: false,
            response_truncated: false,
            is_fallback: false,
            original_model: "gpt-4o".into(),
            attempts_made: 1,
        };
        sink.log(entry);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn test_truncate_bodies_sets_flags() {
        let entry = RequestLog {
            id: Uuid::new_v4(),
            request_id: "req".into(),
            timestamp: Utc::now(),
            virtual_key_id: None,
            key_name: "k".into(),
            provider: "openai".into(),
            provider_name: "p".into(),
            provider_id: None,
            model: "m".into(),
            upstream_model: "m".into(),
            endpoint: "/v1/chat/completions".into(),
            status: 200,
            error_message: String::new(),
            error_type: String::new(),
            is_stream: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            duration_ms: 0,
            first_byte_ms: 0,
            request_body: "a".repeat(100),
            response_body: "b".repeat(50),
            request_truncated: false,
            response_truncated: false,
            is_fallback: false,
            original_model: String::new(),
            attempts_made: 0,
        };

        let truncated = truncate_bodies(entry, 60);
        assert!(truncated.request_truncated);
        assert_eq!(truncated.request_body.len(), 60);
        assert!(!truncated.response_truncated);
        assert_eq!(truncated.response_body.len(), 50);
    }

    #[test]
    fn test_truncate_str_multibyte_no_panic() {
        // Emojis sind 4 Bytes; das Limit liegt mitten in einem Char.
        let mut s = "\u{1F600}".repeat(10); // 40 Bytes
        truncate_str(&mut s, 7); // 7 ist keine Char-Boundary (4er-Multiple)
        assert!(s.len() <= 7);
        assert!(s.is_char_boundary(s.len()));
        assert_eq!(s, "\u{1F600}");

        // via truncate_bodies: Limit mitten in Multi-Byte-Char
        let entry = RequestLog {
            id: Uuid::new_v4(),
            request_id: "req-utf8".into(),
            timestamp: Utc::now(),
            virtual_key_id: None,
            key_name: "k".into(),
            provider: "openai".into(),
            provider_name: "p".into(),
            provider_id: None,
            model: "m".into(),
            upstream_model: "m".into(),
            endpoint: "/v1/chat/completions".into(),
            status: 200,
            error_message: String::new(),
            error_type: String::new(),
            is_stream: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            duration_ms: 0,
            first_byte_ms: 0,
            request_body: "\u{1F600}".repeat(10),
            response_body: String::new(),
            request_truncated: false,
            response_truncated: false,
            is_fallback: false,
            original_model: String::new(),
            attempts_made: 0,
        };
        let truncated = truncate_bodies(entry, 7);
        assert!(truncated.request_truncated);
        assert!(truncated.request_body.len() <= 7);
        // gueltiges UTF-8 + exakt ein ganzer Emoji
        assert_eq!(truncated.request_body, "\u{1F600}");
    }

    fn test_entry(request_id: &str) -> RequestLog {
        RequestLog {
            id: Uuid::new_v4(),
            request_id: request_id.into(),
            timestamp: Utc::now(),
            virtual_key_id: None,
            key_name: "k".into(),
            provider: "openai".into(),
            provider_name: "p".into(),
            provider_id: None,
            model: "m".into(),
            upstream_model: "m".into(),
            endpoint: "/v1/chat/completions".into(),
            status: 200,
            error_message: String::new(),
            error_type: String::new(),
            is_stream: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            cost_usd: 0.0,
            duration_ms: 0,
            first_byte_ms: 0,
            request_body: "payload".repeat(3),
            response_body: String::new(),
            request_truncated: false,
            response_truncated: false,
            is_fallback: false,
            original_model: String::new(),
            attempts_made: 0,
        }
    }

    #[test]
    fn test_overflow_push_drops_oldest_when_full() {
        let overflow = Arc::new(Mutex::new(VecDeque::new()));
        let dropped = Arc::new(AtomicU64::new(0));

        push_overflow(&overflow, &dropped, 2, test_entry("a"));
        push_overflow(&overflow, &dropped, 2, test_entry("b"));
        push_overflow(&overflow, &dropped, 2, test_entry("c"));

        let q = overflow.lock().unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].request_id, "b");
        assert_eq!(q[1].request_id, "c");
        drop(q);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_drain_overflow_preserves_order_and_truncates() {
        let overflow = Arc::new(Mutex::new(VecDeque::new()));
        {
            let mut q = overflow.lock().unwrap();
            q.push_back(test_entry("1"));
            q.push_back(test_entry("2"));
        }
        let mut buffer = Vec::new();
        drain_overflow(&overflow, &mut buffer, 60);

        assert_eq!(overflow.lock().unwrap().len(), 0);
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer[0].request_id, "1");
        assert_eq!(buffer[1].request_id, "2");
    }

    #[test]
    fn test_full_channel_routes_to_overflow_instead_of_drop() {
        // kapazitaet 1: einen Slot fillen, naechster try_send -> Full -> overflow
        let (tx, _rx) = mpsc::channel::<RequestLog>(1);
        let overflow = Arc::new(Mutex::new(VecDeque::new()));
        let dropped = Arc::new(AtomicU64::new(0));
        let sink = LogSink {
            tx,
            live_tx: broadcast::channel::<LiveEvent>(16).0,
            overflow: overflow.clone(),
            dropped: dropped.clone(),
            counters: Arc::new(Mutex::new(HashMap::new())),
            auth_failures: Arc::new(AtomicU64::new(0)),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        };

        // erster Eintrag passt noch in den Channel
        sink.log_only(test_entry("a"));
        assert!(overflow.lock().unwrap().is_empty());

        // zweiter Eintrag: Channel voll -> overflow
        sink.log_only(test_entry("b"));
        let q = overflow.lock().unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].request_id, "b");
        drop(q);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    fn sink_with_counters() -> LogSink {
        let (tx, _rx) = mpsc::channel::<RequestLog>(1000);
        LogSink {
            tx,
            live_tx: broadcast::channel::<LiveEvent>(16).0,
            overflow: Arc::new(Mutex::new(VecDeque::new())),
            dropped: Arc::new(AtomicU64::new(0)),
            counters: Arc::new(Mutex::new(HashMap::new())),
            auth_failures: Arc::new(AtomicU64::new(0)),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn entry_with(status: u16, prompt: u64, completion: u64, cost: f64) -> RequestLog {
        let mut e = test_entry("req-counter");
        e.status = status;
        e.prompt_tokens = prompt;
        e.completion_tokens = completion;
        e.cost_usd = cost;
        e
    }

    #[test]
    fn test_counters_success_error_stream_auth() {
        let sink = sink_with_counters();

        // success (non-stream) via log()
        sink.log(entry_with(200, 10, 5, 0.001));
        // error (>=400, non-auth) via log_only() (stream path)
        sink.log_only(entry_with(500, 10, 0, 0.0));
        // stream success via log_only()
        sink.log_only(entry_with(200, 7, 3, 0.0005));
        // auth failure (401) via log()
        sink.log(entry_with(401, 0, 0, 0.0));

        let (entries, auth_failures) = sink.snapshot_counters();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        // requests = success(1) + error(1) + stream(1) = 3; 401 zaeHLT
        assert_eq!(e.requests, 3);
        // errors = nur der 500
        assert_eq!(e.errors, 1);
        assert_eq!(e.prompt_tokens, 10 + 10 + 7); // 401 zaeHLT (0)
        assert_eq!(e.completion_tokens, 5 + 0 + 3);
        assert_eq!(e.cost_usd_micros, 1000 + 0 + 500);
        assert_eq!(auth_failures, 1);
    }

    #[test]
    fn test_counters_separate_keys() {
        let sink = sink_with_counters();
        let mut a = entry_with(200, 1, 1, 0.0);
        a.provider = "openai".into();
        a.model = "m1".into();
        let mut b = entry_with(400, 2, 2, 0.0);
        b.provider = "anthropic".into();
        b.model = "m2".into();
        sink.log(a);
        sink.log(b);

        let (entries, _auth) = sink.snapshot_counters();
        assert_eq!(entries.len(), 2);
        let openai = entries.iter().find(|e| e.provider == "openai" && e.model == "m1").unwrap();
        let anthropic = entries.iter().find(|e| e.provider == "anthropic" && e.model == "m2").unwrap();
        assert_eq!(openai.requests, 1);
        assert_eq!(openai.errors, 0);
        assert_eq!(anthropic.requests, 1);
        assert_eq!(anthropic.errors, 1);
    }

    #[test]
    fn test_counters_separate_provider_names() {
        let sink = sink_with_counters();
        // gleicher kind+model, zwei Instanzen: werden getrennt gezaehlt
        let mut dgx = entry_with(200, 10, 1, 0.001);
        dgx.provider_name = "DGX Cluster".into();
        let mut dgx2 = entry_with(200, 20, 2, 0.002);
        dgx2.provider_name = "DGX Cluster".into();
        let mut rtx = entry_with(500, 5, 0, 0.0);
        rtx.provider_name = "RTX 3090".into();
        sink.log(dgx);
        sink.log(dgx2);
        sink.log(rtx);

        let (entries, _auth) = sink.snapshot_counters();
        assert_eq!(entries.len(), 2);
        let dgx = entries
            .iter()
            .find(|e| e.provider == "openai" && e.provider_name == "DGX Cluster" && e.model == "m")
            .unwrap();
        assert_eq!(dgx.requests, 2);
        assert_eq!(dgx.errors, 0);
        assert_eq!(dgx.prompt_tokens, 30);
        let rtx = entries
            .iter()
            .find(|e| e.provider == "openai" && e.provider_name == "RTX 3090" && e.model == "m")
            .unwrap();
        assert_eq!(rtx.requests, 1);
        assert_eq!(rtx.errors, 1);
    }

    #[test]
    fn test_dropped_count_getter() {
        let sink = sink_with_counters();
        assert_eq!(sink.dropped_count(), 0);
        sink.dropped.fetch_add(5, Ordering::Relaxed);
        assert_eq!(sink.dropped_count(), 5);
    }

    #[test]
    fn test_counters_empty_provider_falls_back_to_gateway() {
        // Gateway-interner Fehler (error_response): 404 ohne provider/model
        let sink = sink_with_counters();
        let mut e = entry_with(404, 0, 0, 0.0);
        e.provider = String::new();
        e.model = String::new();
        sink.log(e);

        let (entries, auth_failures) = sink.snapshot_counters();
        assert_eq!(auth_failures, 0);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.provider, "gateway");
        assert_eq!(e.model, "");
        assert_eq!(e.requests, 1);
        assert_eq!(e.errors, 1);
    }

    #[test]
    fn test_counters_gateway_fallback_keeps_empty_provider_name() {
        // gateway-fallback: provider wird auf "gateway" gesetzt, provider_name
        // bleibt leer (kein zweiter Fallback).
        let sink = sink_with_counters();
        let mut e = entry_with(502, 0, 0, 0.0);
        e.provider = String::new();
        e.provider_name = String::new();
        e.model = String::new();
        sink.log(e);

        let (entries, _) = sink.snapshot_counters();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.provider, "gateway");
        assert_eq!(e.provider_name, "");
        assert_eq!(e.requests, 1);
        assert_eq!(e.errors, 1);
    }

    #[test]
    fn test_counters_402_counts_as_auth_failure() {
        let sink = sink_with_counters();
        sink.log(entry_with(402, 0, 0, 0.0));

        let (entries, auth_failures) = sink.snapshot_counters();
        assert_eq!(auth_failures, 1);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_cost_micros_rounds_not_truncates() {
        let sink = sink_with_counters();
        // 0.0000005 USD = 0.5 Mikro-USD -> rundet auf 1, nicht 0.
        sink.log(entry_with(200, 0, 0, 0.0000005));

        let (entries, _) = sink.snapshot_counters();
        assert_eq!(entries[0].cost_usd_micros, 1);
    }

    #[test]
    fn test_record_auth_failure_only_increments_auth_failures() {
        let sink = sink_with_counters();
        sink.record_auth_failure();
        sink.record_auth_failure();

        let (entries, auth_failures) = sink.snapshot_counters();
        assert_eq!(auth_failures, 2);
        assert!(entries.is_empty());
    }

    // --- In-flight registry tests ---

    fn started_event(id: &str, ts_millis: i64) -> LiveEvent {
        let ts = DateTime::from_timestamp_millis(ts_millis).unwrap();
        LiveEvent::RequestStarted {
            request_id: id.into(),
            timestamp: ts,
            key_name: "test-key".into(),
            provider: "openai".into(),
            provider_name: "OpenAI Primary".into(),
            model: "gpt-4o".into(),
            endpoint: "/v1/chat/completions".into(),
            is_stream: true,
        }
    }

    fn first_byte_event(id: &str, ms: u64) -> LiveEvent {
        LiveEvent::FirstByte {
            request_id: id.into(),
            first_byte_ms: ms,
        }
    }

    fn completed_event(id: &str) -> LiveEvent {
        LiveEvent::Completed {
            log: LiveLog {
                id: Uuid::new_v4(),
                request_id: id.into(),
                timestamp: Utc::now(),
                key_name: "test-key".into(),
                provider: "openai".into(),
                provider_name: "OpenAI Primary".into(),
                model: "gpt-4o".into(),
                upstream_model: "gpt-4o".into(),
                endpoint: "/v1/chat/completions".into(),
                status: 200,
                error_type: String::new(),
                is_stream: true,
                prompt_tokens: 10,
                completion_tokens: 5,
                cost_usd: 0.001,
                duration_ms: 200,
                first_byte_ms: 50,
                is_fallback: false,
                original_model: "gpt-4o".into(),
                attempts_made: 1,
            },
        }
    }

    #[test]
    fn test_inflight_insert_on_started() {
        let sink = sink_with_counters();
        let now_ms = Utc::now().timestamp_millis();
        sink.emit(started_event("req-1", now_ms));

        let snapshot = sink.in_flight_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].request_id, "req-1");
        assert_eq!(snapshot[0].provider, "openai");
        assert_eq!(snapshot[0].provider_name, "OpenAI Primary");
        assert_eq!(snapshot[0].model, "gpt-4o");
        assert_eq!(snapshot[0].key_name, "test-key");
        assert_eq!(snapshot[0].first_byte_ms, None);
    }

    #[test]
    fn test_inflight_first_byte_sets_value() {
        let sink = sink_with_counters();
        let now_ms = Utc::now().timestamp_millis();
        sink.emit(started_event("req-1", now_ms));
        sink.emit(first_byte_event("req-1", 42));

        let snapshot = sink.in_flight_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].first_byte_ms, Some(42));
    }

    #[test]
    fn test_inflight_remove_on_completed() {
        let sink = sink_with_counters();
        let now_ms = Utc::now().timestamp_millis();
        sink.emit(started_event("req-1", now_ms));
        sink.emit(first_byte_event("req-1", 42));
        sink.emit(completed_event("req-1"));

        let snapshot = sink.in_flight_snapshot();
        assert!(snapshot.is_empty());
    }

    #[test]
    fn test_inflight_noop_remove_unknown() {
        let sink = sink_with_counters();
        // Completed fuer einen unbekannten request_id: darf nicht panicen
        sink.emit(completed_event("unknown-req"));

        let snapshot = sink.in_flight_snapshot();
        assert!(snapshot.is_empty());
    }

    #[test]
    fn test_inflight_ttl_sweep_removes_old() {
        let sink = sink_with_counters();
        // Eintrag 20 Minuten alt (ueber 15 Min TTL)
        let old_ms = Utc::now().timestamp_millis() - (20 * 60 * 1000);
        sink.emit(started_event("req-old", old_ms));
        // Eintrag frisch (soeben)
        let now_ms = Utc::now().timestamp_millis();
        sink.emit(started_event("req-new", now_ms));

        let snapshot = sink.in_flight_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].request_id, "req-new");
    }

    #[test]
    fn test_inflight_sorted_by_started_at() {
        let sink = sink_with_counters();
        let base = Utc::now().timestamp_millis();
        sink.emit(started_event("req-c", base + 2000));
        sink.emit(started_event("req-a", base));
        sink.emit(started_event("req-b", base + 1000));

        let snapshot = sink.in_flight_snapshot();
        assert_eq!(snapshot.len(), 3);
        assert_eq!(snapshot[0].request_id, "req-a");
        assert_eq!(snapshot[1].request_id, "req-b");
        assert_eq!(snapshot[2].request_id, "req-c");
    }

    #[test]
    fn test_log_removes_inflight_registry_entry() {
        let sink = sink_with_counters();
        let now_ms = Utc::now().timestamp_millis();
        sink.emit(started_event("req-42", now_ms));

        // Ensure entry exists
        let snapshot = sink.in_flight_snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].request_id, "req-42");

        // log() should remove the in-flight entry via the Completed event
        let entry = test_entry("req-42");
        sink.log(entry);

        let snapshot = sink.in_flight_snapshot();
        assert!(snapshot.is_empty());
    }
}
