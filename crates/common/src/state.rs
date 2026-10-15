//! AppState: DB-Pools, HTTP-Client, Key-Cache und Model-Routing-Tabelle.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use ingest::LogSink;
use providers::{ProviderKind, RouteTarget};

/// Cache fuer die /metrics-Sektionen (Latenz 24h, Live 60s, Upstream-Engines).
/// Wird alle 30s von einem Background-Task in yalr/main.rs erneuert; die
/// Sektionen sind unabhaengig (eigener Build-Zeitpunkt, eigener Fehlerfall).
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    /// Fertig gerenderte Prometheus-Textzeilen der 24h-Latenz-Sektion.
    pub latency_body: String,
    /// Zeitpunkt des letzten erfolgreichen Baus der Latenz-Sektion (fuer Age-Metrik).
    pub latency_built_at: Option<DateTime<Utc>>,
    /// Fertig gerenderte Prometheus-Textzeilen der 60s-Live-Sektion.
    pub live_body: String,
    /// Zeitpunkt des letzten erfolgreichen Baus der Live-Sektion (fuer Age-Metrik).
    pub live_built_at: Option<DateTime<Utc>>,
    /// Fertig gerenderte Prometheus-Textzeilen der Upstream-Engine-Sektion.
    pub upstream_body: String,
    /// Zeitpunkt des letzten Baus der Upstream-Sektion (fuer Age-Metrik).
    pub upstream_built_at: Option<DateTime<Utc>>,
    /// true nur, wenn BEIDE CH-abhaengigen Sektionen (Latenz + Live) in dem
    /// aktuellen Zyklus gebaut wurden; false, wenn mindestens eine fehlgeschlagen
    /// ist (AND-Kombination, kein Last-Write-Wins). None = noch nie gebaut
    /// (dann wird die Metric-Zeile nicht gerendert, statt eine falsche 0).
    pub clickhouse_up: Option<bool>,
}

/// Feste ID des synthetischen Keys, mit dem der Dashboard-Playground Requests
/// durch die Proxy-Pipeline schickt (kein DB-Eintrag; in Logs erkennbar).
pub const PLAYGROUND_KEY_ID: Uuid = Uuid::from_u128(0x706c6179_6772_6f75_6e64_000000000001);
/// Name des Playground-Keys; fuer echte Keys reserviert.
pub const PLAYGROUND_KEY_NAME: &str = "playground";

/// Ein Virtual Key (aus Postgres, gecached).
#[derive(Debug, Clone)]
pub struct VirtualKey {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub budget_cents: Option<i64>,
    pub enabled: bool,
    /// Allow-List provider_id; leer = unbeschränkt.
    pub allowed_providers: Vec<Uuid>,
    /// Allow-List angefragter Modellnamen (Client-Sicht); leer = unbeschränkt.
    pub allowed_models: Vec<String>,
}

/// Ausgaben eines Keys (aus ClickHouse agregiert, periodisch aktualisiert).
#[derive(Debug, Clone, Default)]
pub struct KeySpend {
    pub cost_usd: f64,
    pub updated_at: Option<Instant>,
}

pub struct AppStateInner {
    pub pg: sqlx::PgPool,
    pub ch: clickhouse::Client,
    pub http: reqwest::Client,
    pub log_sink: LogSink,
    pub session_secret: String,
    pub encryption_key: String,

    /// key_hash -> VirtualKey (cache, 60s TTL)
    pub key_cache: RwLock<HashMap<String, (VirtualKey, Instant)>>,
    /// Routing: model_name -> RouteTargets (primär + fallbacks, sortiert)
    pub routes: RwLock<HashMap<String, Vec<RouteTarget>>>,
    /// key_id -> kumulierte Ausgaben (für Budget-Check)
    pub key_spend: RwLock<HashMap<Uuid, KeySpend>>,
    /// Fallback-Definitionen: model_name -> [fallback model names]
    pub fallbacks: RwLock<HashMap<String, Vec<String>>>,
    /// Temporäre, absichtsvolle Redirects: model_name -> redirect_model_name.
    /// Greifen immer (unabhängig von Ziels Gesundheit), max. EIN Hop.
    pub redirects: RwLock<HashMap<String, String>>,
    /// Cache fuer CH-Latenz-Quantile (Prometheus-Snapshot).
    pub metrics_snapshot: Arc<RwLock<MetricsSnapshot>>,
    /// Optionaler Token fuer GET /metrics (None = Endpoint offen).
    pub metrics_token: Option<String>,
    /// Lastverteilung: laufende Anfragen je Target (s. `target_key`). Haben
    /// mehrere primäre Targets eines Modells `"load_balance": true` in den
    /// Capabilities, geht die Anfrage an das mit den wenigsten.
    pub inflight: Mutex<HashMap<String, Arc<AtomicUsize>>>,
    /// Lastverteilung: Reihum-Zähler je Modell – entscheidet bei Gleichstand.
    pub round_robin: Mutex<HashMap<String, usize>>,
}

/// Zählt eine laufende Anfrage an einem Target, bis er fallen gelassen wird
/// (bei Streams erst, wenn der Strom zu Ende ist).
pub struct InflightGuard(Arc<AtomicUsize>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Nimmt das Target an der Lastverteilung teil? (`"load_balance": true` in den Capabilities)
fn load_balance(t: &RouteTarget) -> bool {
    t.capabilities
        .as_ref()
        .and_then(|c| c.get("load_balance"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Schlüssel eines Targets für die Lastverteilung: Provider + Upstream-Modell.
fn target_key(t: &RouteTarget) -> String {
    format!("{}|{}", t.provider_id, t.upstream_model)
}

pub type AppState = Arc<AppStateInner>;

/// Ergebnis von `resolve_route`: der effektive Modellname (nach max. EINEM
/// Redirect-Hop) und die aufgelösten Ziel-Targets (primär + Fallbacks des
/// ZIELS). `targets` ist `None`, wenn für den effektiven Namen keine Route
/// existiert (u.a. der "tote Redirect"-Fall).
pub struct ResolvedRoute {
    pub effective: String,
    pub targets: Option<Vec<RouteTarget>>,
}

impl AppStateInner {
    pub fn new(
        pg: sqlx::PgPool,
        ch: clickhouse::Client,
        http: reqwest::Client,
        log_sink: LogSink,
        session_secret: String,
        encryption_key: String,
        metrics_snapshot: Arc<RwLock<MetricsSnapshot>>,
        metrics_token: Option<String>,
    ) -> Self {
        Self {
            pg,
            ch,
            http,
            log_sink,
            session_secret,
            encryption_key,
            key_cache: RwLock::new(HashMap::new()),
            routes: RwLock::new(HashMap::new()),
            key_spend: RwLock::new(HashMap::new()),
            fallbacks: RwLock::new(HashMap::new()),
            redirects: RwLock::new(HashMap::new()),
            metrics_snapshot,
            metrics_token,
            inflight: Mutex::new(HashMap::new()),
            round_robin: Mutex::new(HashMap::new()),
        }
    }

    fn inflight_counter(&self, t: &RouteTarget) -> Arc<AtomicUsize> {
        let mut map = self.inflight.lock().unwrap();
        map.entry(target_key(t)).or_default().clone()
    }

    /// Laufende Anfragen an einem Target (für Lastverteilung und Tests).
    pub fn inflight_count(&self, t: &RouteTarget) -> usize {
        self.inflight_counter(t).load(Ordering::Relaxed)
    }

    /// Meldet eine Anfrage an einem Target an; zählt, bis der Guard fällt.
    pub fn inflight_begin(&self, t: &RouteTarget) -> InflightGuard {
        let c = self.inflight_counter(t);
        c.fetch_add(1, Ordering::Relaxed);
        InflightGuard(c)
    }

    /// Ordnet die primären Targets eines Modells. Nur Targets mit
    /// `"load_balance": true` werden verteilt: wenigste laufende Anfragen
    /// zuerst, bei Gleichstand reihum (Startpunkt wandert je Anfrage weiter).
    /// Sie kommen nach vorn, die übrigen behalten ihre Reihenfolge dahinter
    /// (Ausfall-Reserve wie bisher). Weniger als zwei verteilte Targets: alles
    /// unverändert – mehrere gleichnamige Targets bleiben sonst eine Reihenfolge.
    fn order_primaries(&self, model_name: &str, primary: &mut Vec<RouteTarget>) {
        if primary.iter().filter(|t| load_balance(t)).count() < 2 {
            return;
        }
        let (mut verteilt, rest): (Vec<RouteTarget>, Vec<RouteTarget>) =
            primary.drain(..).partition(load_balance);
        let start = {
            let mut rr = self.round_robin.lock().unwrap();
            let n = rr.entry(model_name.to_string()).or_insert(0);
            let start = *n % verteilt.len();
            *n = n.wrapping_add(1);
            start
        };
        verteilt.rotate_left(start);
        // stabil: Gleichstand behält die Reihum-Reihenfolge
        verteilt.sort_by_key(|t| self.inflight_count(t));
        primary.extend(verteilt);
        primary.extend(rest);
    }

    /// Laedt alle Routen (models + providers) aus Postgres neu.
    pub async fn reload_routes(&self) -> anyhow::Result<()> {
        #[derive(sqlx::FromRow)]
        struct RouteRow {
            model_name: String,
            upstream_model: String,
            input_price_per_million: f64,
            output_price_per_million: f64,
            provider_id: Uuid,
            provider_name: String,
            provider_kind: String,
            base_url: String,
            api_key_encrypted: String,
            capabilities: Option<serde_json::Value>,
        }

        let rows = sqlx::query_as::<_, RouteRow>(
            r#"
            SELECT m.model_name, m.upstream_model,
                   m.input_price_per_million::float8 AS input_price_per_million,
                   m.output_price_per_million::float8 AS output_price_per_million,
                   p.id AS provider_id, p.name AS provider_name, p.kind AS provider_kind,
                   p.base_url, p.api_key_encrypted, m.capabilities
            FROM models m
            JOIN providers p ON p.id = m.provider_id
            WHERE m.enabled = TRUE AND p.enabled = TRUE
            ORDER BY m.created_at
            "#,
        )
        .fetch_all(&self.pg)
        .await?;

        let mut routes: HashMap<String, Vec<RouteTarget>> = HashMap::new();
        for row in rows {
            let Ok(kind) = ProviderKind::parse(&row.provider_kind) else {
                tracing::warn!("skipping model {} with unknown provider kind {}", row.model_name, row.provider_kind);
                continue;
            };
            let api_key = crate::crypto::decrypt(&row.api_key_encrypted, &self.encryption_key)?;
            let target = RouteTarget {
                provider_id: row.provider_id,
                provider_name: row.provider_name,
                provider_kind: kind,
                base_url: row.base_url,
                api_key,
                model_name: row.model_name.clone(),
                upstream_model: row.upstream_model,
                input_price_per_million: row.input_price_per_million,
                output_price_per_million: row.output_price_per_million,
                capabilities: row.capabilities,
            };
            routes.entry(row.model_name).or_default().push(target);
        }

        // Fallbacks laden
        #[derive(sqlx::FromRow)]
        struct FbRow {
            model_name: String,
            fallback_model_name: String,
        }
        let fb_rows = sqlx::query_as::<_, FbRow>(
            r#"
            SELECT model_name, fallback_model_name
            FROM fallbacks
            WHERE enabled = TRUE
            ORDER BY priority ASC
            "#,
        )
        .fetch_all(&self.pg)
        .await?;

        let mut fallbacks: HashMap<String, Vec<String>> = HashMap::new();
        for row in fb_rows {
            fallbacks.entry(row.model_name).or_default().push(row.fallback_model_name);
        }

        // Redirects laden (model_name -> redirect_model_name)
        #[derive(sqlx::FromRow)]
        struct RedirectRow {
            model_name: String,
            redirect_model_name: String,
        }
        let redirect_rows = sqlx::query_as::<_, RedirectRow>(
            r#"
            SELECT model_name, redirect_model_name
            FROM redirects
            "#,
        )
        .fetch_all(&self.pg)
        .await?;

        let mut redirects: HashMap<String, String> = HashMap::new();
        for row in redirect_rows {
            redirects.insert(row.model_name, row.redirect_model_name);
        }

        // Lock-Beschaffungs-Reihenfolge konsistent halten:
        // redirects -> routes -> fallbacks.
        let mut redirect_guard = self.redirects.write().await;
        *redirect_guard = redirects;
        let mut routes_guard = self.routes.write().await;
        *routes_guard = routes;
        let mut fb_guard = self.fallbacks.write().await;
        *fb_guard = fallbacks;
        tracing::info!("routes loaded: {} models, {} redirects", routes_guard.len(), redirect_guard.len());
        Ok(())
    }

    /// Loest einen angefragten Model-Namen in effektiven Namen + Targets auf.
    ///
    /// 1. Redirect: `effective = redirects[requested]` — genau EIN Hop, Redirects
    ///    werden NICHT verkettet (wird das Ziel selbst redirectet, folgt das
    ///    hier nicht weiter).
    /// 2. Targets: primär `routes[effective]` + für jeden Namen in
    ///    `fallbacks[effective]` deren `routes` (Bestehende Fallback-Logik,
    ///    aber auf `effective` statt `requested` angewandt). Mehrere primäre
    ///    Targets werden nach Last geordnet (`order_primaries`).
    pub async fn resolve_route(&self, model_name: &str) -> ResolvedRoute {
        let redirects = self.redirects.read().await;
        let routes = self.routes.read().await;
        let fallbacks = self.fallbacks.read().await;

        // max. EIN Hop: Redirect auf das Ziel des Redirects wird nicht gefolgt.
        let effective = redirects
            .get(model_name)
            .cloned()
            .unwrap_or_else(|| model_name.to_string());

        let mut result: Vec<RouteTarget> = Vec::new();
        if let Some(primary) = routes.get(&effective) {
            result.extend(primary.iter().cloned());
            // mehrere primäre Targets: Last verteilen (Fallbacks bleiben dahinter)
            self.order_primaries(&effective, &mut result);
        }
        if let Some(fb_names) = fallbacks.get(&effective) {
            for fb_name in fb_names {
                if let Some(targets) = routes.get(fb_name) {
                    result.extend(targets.iter().cloned());
                }
            }
        }

        let targets = if result.is_empty() { None } else { Some(result) };
        ResolvedRoute { effective, targets }
    }

    /// VirtualKey anhand des Key-Hashes ermitteln (mit Cache).
    pub async fn lookup_key(&self, key_hash: &str) -> Option<VirtualKey> {
        const TTL: Duration = Duration::from_secs(60);

        // cache-hit?
        {
            let cache = self.key_cache.read().await;
            if let Some((vk, at)) = cache.get(key_hash) {
                if at.elapsed() < TTL {
                    return Some(vk.clone());
                }
            }
        }

        // db-lookup
        #[derive(sqlx::FromRow)]
        struct KeyRow {
            id: Uuid,
            name: String,
            key_hash: String,
            budget_cents: Option<i64>,
            enabled: bool,
        }
        let row = sqlx::query_as::<_, KeyRow>(
            r#"
            SELECT id, name, key_hash, budget_cents, enabled
            FROM virtual_keys
            WHERE key_hash = $1
            "#,
        )
        .bind(key_hash)
        .fetch_optional(&self.pg)
        .await
        .ok()
        .flatten()?;

        if !row.enabled {
            return None;
        }

        // Key-Scopes (leer = unbeschränkt in der jeweiligen Dimension).
        // Fehlerbehandlung fail-closed wie das Haupt-Query: DB-Fehler -> Key
        // nicht verwenden (401), statt still unbeschränkt zu werden.
        let allowed_providers: Vec<Uuid> = match sqlx::query_scalar(
            "SELECT provider_id FROM key_providers WHERE virtual_key_id = $1",
        )
        .bind(row.id)
        .fetch_all(&self.pg)
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(key_id = %row.id, error = %e, "failed to load key provider scope; denying key");
                return None;
            }
        };
        let allowed_models: Vec<String> = match sqlx::query_scalar(
            "SELECT model_name FROM key_models WHERE virtual_key_id = $1",
        )
        .bind(row.id)
        .fetch_all(&self.pg)
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(key_id = %row.id, error = %e, "failed to load key model scope; denying key");
                return None;
            }
        };

        let vk = VirtualKey {
            id: row.id,
            name: row.name,
            key_hash: row.key_hash,
            budget_cents: row.budget_cents,
            enabled: row.enabled,
            allowed_providers,
            allowed_models,
        };

        let mut cache = self.key_cache.write().await;
        cache.insert(key_hash.to_string(), (vk.clone(), Instant::now()));
        Some(vk)
    }

    /// Prueft ob das Budget eines Keys erschoepft ist.
    pub async fn is_budget_exceeded(&self, key: &VirtualKey) -> bool {
        let Some(budget_cents) = key.budget_cents else {
            return false;
        };
        let budget_usd = budget_cents as f64 / 100.0;

        // frische cache-werte?
        {
            let spend = self.key_spend.read().await;
            if let Some(s) = spend.get(&key.id) {
                if let Some(at) = s.updated_at {
                    if at.elapsed() < Duration::from_secs(30) {
                        return s.cost_usd >= budget_usd;
                    }
                }
            }
        }

        // aus ClickHouse nachladen
        let cost = self
            .ch
            .query("SELECT sum(cost_usd) FROM yalr.request_logs WHERE virtual_key_id = ?")
            .bind(key.id)
            .fetch_one::<f64>()
            .await
            .unwrap_or(0.0);

        let mut spend = self.key_spend.write().await;
        spend.insert(
            key.id,
            KeySpend {
                cost_usd: cost,
                updated_at: Some(Instant::now()),
            },
        );
        cost >= budget_usd
    }

    /// Aktualisiert die lokale Spend-Statistik nach jedem Request (async, fire-and-forget).
    pub fn track_spend(self: &Arc<Self>, key_id: Uuid, cost_delta: f64) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            let mut guard = state.key_spend.write().await;
            let entry = guard.entry(key_id).or_default();
            entry.cost_usd += cost_delta;
            entry.updated_at = Some(Instant::now());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_target(model: &str) -> RouteTarget {
        RouteTarget {
            provider_id: Uuid::new_v4(),
            provider_name: "p".into(),
            provider_kind: ProviderKind::OpenAi,
            base_url: "http://localhost".into(),
            api_key: "k".into(),
            model_name: model.into(),
            upstream_model: model.into(),
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            capabilities: None,
        }
    }

    /// Baut einen Stateless-State mit Lazy-Pools (werden nie angeruehrt) und
    /// fueellt die drei Routing-Maps direkt.
    async fn state_with(
        routes: HashMap<String, Vec<RouteTarget>>,
        fallbacks: HashMap<String, Vec<String>>,
        redirects: HashMap<String, String>,
    ) -> AppState {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://user:pass@127.0.0.1:1/none")
            .unwrap();
        let (sink, _handle) = ingest::start(
            clickhouse::Client::default(),
            ingest::IngestConfig::default(),
        );
        let state = Arc::new(AppStateInner::new(
            pg,
            clickhouse::Client::default(),
            reqwest::Client::new(),
            sink,
            "session-secret".into(),
            "0".repeat(32),
            Arc::new(tokio::sync::RwLock::new(MetricsSnapshot::default())),
            None,
        ));
        *state.routes.write().await = routes;
        *state.fallbacks.write().await = fallbacks;
        *state.redirects.write().await = redirects;
        state
    }

    fn model_names(targets: &Option<Vec<RouteTarget>>) -> Vec<String> {
        targets
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.model_name)
            .collect()
    }

    #[tokio::test]
    async fn test_resolve_no_redirect_unchanged() {
        let mut routes = HashMap::new();
        routes.insert("A".to_string(), vec![mk_target("A")]);
        let mut fb = HashMap::new();
        fb.insert("A".to_string(), vec!["B".to_string()]);
        routes.insert("B".to_string(), vec![mk_target("B")]);

        let state = state_with(routes, fb, HashMap::new()).await;
        let resolved = state.resolve_route("A").await;
        assert_eq!(resolved.effective, "A");
        // altes Verhalten: primär A + Fallback B
        assert_eq!(model_names(&resolved.targets), vec!["A".to_string(), "B".to_string()]);
    }

    #[tokio::test]
    async fn test_resolve_redirect_applied() {
        let mut routes = HashMap::new();
        routes.insert("A".to_string(), vec![mk_target("A")]);
        routes.insert("B".to_string(), vec![mk_target("B")]);

        let mut redirects = HashMap::new();
        redirects.insert("A".to_string(), "B".to_string());

        let state = state_with(routes, HashMap::new(), redirects).await;
        let resolved = state.resolve_route("A").await;
        assert_eq!(resolved.effective, "B");
        // Redirect-Ziel bedient: nur Bs Routes (As eigene nicht mehr)
        assert_eq!(model_names(&resolved.targets), vec!["B".to_string()]);
    }

    #[tokio::test]
    async fn test_resolve_redirect_single_hop() {
        // A -> B und B -> C: A loest auf B auf, NICHT weiter auf C.
        let mut routes = HashMap::new();
        routes.insert("B".to_string(), vec![mk_target("B")]);
        routes.insert("C".to_string(), vec![mk_target("C")]);

        let mut redirects = HashMap::new();
        redirects.insert("A".to_string(), "B".to_string());
        redirects.insert("B".to_string(), "C".to_string());

        let state = state_with(routes, HashMap::new(), redirects).await;
        let resolved = state.resolve_route("A").await;
        assert_eq!(resolved.effective, "B");
        assert_eq!(model_names(&resolved.targets), vec!["B".to_string()]);
    }

    #[tokio::test]
    async fn test_resolve_redirect_uses_target_fallbacks() {
        // A -> B. Fallbacks von B greifen, Fallbacks von A NICHT.
        let mut routes = HashMap::new();
        routes.insert("A".to_string(), vec![mk_target("A")]);
        routes.insert("B".to_string(), vec![mk_target("B")]);
        routes.insert("C".to_string(), vec![mk_target("C")]);
        routes.insert("D".to_string(), vec![mk_target("D")]);

        let mut fb = HashMap::new();
        fb.insert("B".to_string(), vec!["C".to_string()]);
        fb.insert("A".to_string(), vec!["D".to_string()]); // darf nicht greifen

        let mut redirects = HashMap::new();
        redirects.insert("A".to_string(), "B".to_string());

        let state = state_with(routes, fb, redirects).await;
        let resolved = state.resolve_route("A").await;
        assert_eq!(resolved.effective, "B");
        // B + Bs Fallback C; As Fallback D fehlt
        assert_eq!(model_names(&resolved.targets), vec!["B".to_string(), "C".to_string()]);
    }

    #[tokio::test]
    async fn test_resolve_dead_redirect_has_no_targets() {
        // Redirect A -> B, aber B hat keine Route: effective=B, targets=None
        // (der 503-"toter Redirect"-Fall).
        let mut routes = HashMap::new();
        routes.insert("A".to_string(), vec![mk_target("A")]);

        let mut redirects = HashMap::new();
        redirects.insert("A".to_string(), "B".to_string());

        let state = state_with(routes, HashMap::new(), redirects).await;
        let resolved = state.resolve_route("A").await;
        assert_eq!(resolved.effective, "B");
        assert!(resolved.targets.is_none());
    }

    fn mk_named(model: &str, provider: &str) -> RouteTarget {
        RouteTarget {
            provider_name: provider.into(),
            capabilities: Some(serde_json::json!({"load_balance": true})),
            ..mk_target(model)
        }
    }

    fn provider_names(targets: &Option<Vec<RouteTarget>>) -> Vec<String> {
        targets.clone().unwrap_or_default().into_iter().map(|t| t.provider_name).collect()
    }

    #[tokio::test]
    async fn test_mehrere_primaere_reihum_fallback_bleibt_hinten() {
        let mut routes = HashMap::new();
        routes.insert("V".to_string(), vec![mk_named("V", "p1"), mk_named("V", "p2"), mk_named("V", "p3")]);
        routes.insert("F".to_string(), vec![mk_named("F", "fb")]);
        let mut fb = HashMap::new();
        fb.insert("V".to_string(), vec!["F".to_string()]);
        let state = state_with(routes, fb, HashMap::new()).await;

        let mut erste = Vec::new();
        for _ in 0..6 {
            let r = state.resolve_route("V").await;
            let namen = provider_names(&r.targets);
            assert_eq!(namen.len(), 4);
            assert_eq!(namen[3], "fb", "Fallback immer zuletzt");
            erste.push(namen[0].clone());
        }
        assert_eq!(erste, vec!["p1", "p2", "p3", "p1", "p2", "p3"]);
    }

    #[tokio::test]
    async fn test_wenigste_laufende_anfragen_zuerst() {
        let mut routes = HashMap::new();
        let (a, b) = (mk_named("V", "a"), mk_named("V", "b"));
        routes.insert("V".to_string(), vec![a.clone(), b.clone()]);
        let state = state_with(routes, HashMap::new(), HashMap::new()).await;

        // a ist beschäftigt: b gewinnt, egal wo der Reihum-Zähler steht
        let g1 = state.inflight_begin(&a);
        let g2 = state.inflight_begin(&a);
        assert_eq!(state.inflight_count(&a), 2);
        for _ in 0..4 {
            assert_eq!(provider_names(&state.resolve_route("V").await.targets)[0], "b");
        }
        // b bekommt mehr als a: jetzt wieder a
        let _g3 = state.inflight_begin(&b);
        let _g4 = state.inflight_begin(&b);
        let _g5 = state.inflight_begin(&b);
        assert_eq!(provider_names(&state.resolve_route("V").await.targets)[0], "a");
        // Guards fallen: Zähler zurück
        drop(g1);
        drop(g2);
        assert_eq!(state.inflight_count(&a), 0);
        assert_eq!(state.inflight_count(&b), 3);
    }

    #[tokio::test]
    async fn test_ohne_schalter_bleibt_reihenfolge() {
        // gleichnamige Targets ohne "load_balance": erstes ist primär, Rest Reserve – wie bisher
        let mut routes = HashMap::new();
        let mut reserve = mk_named("M", "teuer");
        reserve.capabilities = None;
        let mut erstes = mk_named("M", "haupt");
        erstes.capabilities = Some(serde_json::json!({"attachment": true}));
        routes.insert("M".to_string(), vec![erstes, reserve]);
        let state = state_with(routes, HashMap::new(), HashMap::new()).await;
        for _ in 0..4 {
            assert_eq!(provider_names(&state.resolve_route("M").await.targets), vec!["haupt", "teuer"]);
        }
    }

    #[tokio::test]
    async fn test_gemischt_verteilte_vorn_reserve_dahinter() {
        let mut routes = HashMap::new();
        let mut reserve = mk_named("V", "jazz");
        reserve.capabilities = None;
        routes.insert("V".to_string(), vec![reserve, mk_named("V", "i1"), mk_named("V", "i2")]);
        let state = state_with(routes, HashMap::new(), HashMap::new()).await;
        let a = provider_names(&state.resolve_route("V").await.targets);
        let b = provider_names(&state.resolve_route("V").await.targets);
        assert_eq!(a, vec!["i1", "i2", "jazz"]);
        assert_eq!(b, vec!["i2", "i1", "jazz"]);
    }

    #[tokio::test]
    async fn test_ein_primaeres_target_unveraendert() {
        let mut routes = HashMap::new();
        routes.insert("A".to_string(), vec![mk_named("A", "x")]);
        let state = state_with(routes, HashMap::new(), HashMap::new()).await;
        for _ in 0..3 {
            assert_eq!(provider_names(&state.resolve_route("A").await.targets), vec!["x"]);
        }
        assert!(state.round_robin.lock().unwrap().is_empty(), "kein Zähler für Einzel-Targets");
    }
}
