//! AppState: DB-Pools, HTTP-Client, Key-Cache und Model-Routing-Tabelle.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use ingest::LogSink;
use providers::{ProviderKind, RouteTarget};

/// Cache fuer die CH-abhaengigen /metrics-Anteile (Latenz-Quantile).
/// Wird alle 30s von einem Background-Task in yalr/main.rs erneuert.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    /// Fertig gerenderte Prometheus-Textzeilen der CH-abhaengigen Metriken.
    pub exposition_body: String,
    /// true/false nach dem ersten Snapshot-Bau; None = noch nie gebaut
    /// (dann wird die Metric-Zeile nicht gerendert, statt eine falsche 0).
    pub clickhouse_up: Option<bool>,
    /// Zeitpunkt des letzten erfolgreichen Baus (fuer Age-Metrik).
    pub built_at: Option<DateTime<Utc>>,
}

/// Ein Virtual Key (aus Postgres, gecached).
#[derive(Debug, Clone)]
pub struct VirtualKey {
    pub id: Uuid,
    pub name: String,
    pub key_hash: String,
    pub budget_cents: Option<i64>,
    pub enabled: bool,
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
    /// Cache fuer CH-Latenz-Quantile (Prometheus-Snapshot).
    pub metrics_snapshot: Arc<RwLock<MetricsSnapshot>>,
    /// Optionaler Token fuer GET /metrics (None = Endpoint offen).
    pub metrics_token: Option<String>,
}

pub type AppState = Arc<AppStateInner>;

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
            metrics_snapshot,
            metrics_token,
        }
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

        let mut routes_guard = self.routes.write().await;
        *routes_guard = routes;
        let mut fb_guard = self.fallbacks.write().await;
        *fb_guard = fallbacks;
        tracing::info!("routes loaded: {} models", routes_guard.len());
        Ok(())
    }

    /// Liefert die Route-Ziele fuer einen Model-Namen (primär + fallbacks aufgeloest).
    pub async fn resolve_route(&self, model_name: &str) -> Option<Vec<RouteTarget>> {
        let routes = self.routes.read().await;
        let fallbacks = self.fallbacks.read().await;

        let mut result = Vec::new();
        if let Some(primary) = routes.get(model_name) {
            result.extend(primary.iter().cloned());
        }
        if let Some(fb_names) = fallbacks.get(model_name) {
            for fb_name in fb_names {
                if let Some(targets) = routes.get(fb_name) {
                    result.extend(targets.iter().cloned());
                }
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
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

        let vk = VirtualKey {
            id: row.id,
            name: row.name,
            key_hash: row.key_hash,
            budget_cents: row.budget_cents,
            enabled: row.enabled,
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
