use yalr::config::Config;
use yalr::router::build_router;
use common::state::{AppState, AppStateInner, MetricsSnapshot};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,yalr=debug".into()),
        )
        .init();

    let config = Config::from_env()?;
    tracing::info!("starting yalr on {}:{}", config.host, config.port);

    // Postgres verbinden + migrationen
    let pg = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.postgres_url)
        .await?;
    sqlx::migrate!("../../migrations").run(&pg).await?;
    tracing::info!("postgres migrations applied");

    // Admin-User bootstrap
    dashboard::auth::ensure_admin_user(&pg, &config.admin_username, &config.admin_password).await?;

    // ClickHouse client + schema.
    // ensure_schema braucht einen Client OHNE with_database (siehe Doku dort);
    // die Queries im App-Betrieb nutzen dann den Client mit gesetzter DB.
    let ch = clickhouse::Client::default()
        .with_url(config.clickhouse_url.clone())
        .with_user(config.clickhouse_user.clone())
        .with_password(config.clickhouse_password.clone());
    ingest::ensure_schema(&ch).await?;
    tracing::info!("clickhouse schema ensured");
    let ch = ch.with_database(config.clickhouse_db.clone());

    // Metrik-Snapshot-Store (wird vom Background-Task alle 30s erneuert)
    let metrics_snapshot = Arc::new(tokio::sync::RwLock::new(MetricsSnapshot::default()));
    let metrics_token = std::env::var("METRICS_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());

    // Log-ingest starten
    let (sink, _ingest_handle) = ingest::start(
        ch.clone(),
        ingest::IngestConfig {
            body_max_len: config.log_body_max_len,
            ..Default::default()
        },
    );

    // Kein gesamt-Timeout: laengere LLM-streams duerfen nicht abgebrochen
    // werden. connect_timeout begrenzt nur die Verbindungsaufnahme.
    // read_timeout ist ein per-read-stillstands-schutz (resettet nach jedem
    // empfangenen byte; 300s ohne neues byte = abbruch, der request wird dann
    // kontrolliert terminiert und mit partial-daten geloggt). Wichtig:
    // read_timeout deckt auch das Warten auf die ERSTEN response-bytes ab
    // (First-Byte-Frist = read_timeout). 300s, weil bei stark ausgelasteten
    // lokalen Providern lange Queue/Non-Stream-Prefill dauern kann.
    let http_client = reqwest::Client::builder()
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(300))
        .build()?;

    let state: AppState = Arc::new(AppStateInner::new(
        pg.clone(),
        ch.clone(),
        http_client,
        sink,
        config.session_secret.clone(),
        config.encryption_key.clone(),
        metrics_snapshot.clone(),
        metrics_token,
    ));

    // Routen initial laden + periodisch neu laden
    state.reload_routes().await?;
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                if let Err(e) = state.reload_routes().await {
                    tracing::error!("route reload failed: {e}");
                }
            }
        });
    }

    // Metrik-Snapshot-Task: alle 30s die drei Sektionen (Latenz 24h, Live 60s,
    // Upstream) unabhängig voneinander bauen und in den Snapshot-Store
    // schreiben. Handle wird wie der Ingest-Handle geboxt und gedroppt
    // (kein graceful Shutdown noetig).
    let state_snapshot = state.clone();
    let _metrics_handle = Box::new(tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;

            // a) Latenz-Sektion (24h): bei CH-Fehler alten Body behalten
            let latency_ok = match dashboard::handlers::build_latency_body(&state_snapshot).await {
                Ok(body) => {
                    let mut snap = state_snapshot.metrics_snapshot.write().await;
                    snap.latency_body = body;
                    snap.latency_built_at = Some(chrono::Utc::now());
                    true
                }
                Err(e) => {
                    tracing::warn!("latency metrics query failed, keeping previous: {e}");
                    false
                }
            };

            // b) Live-Sektion (60s): bei CH-Fehler Body leeren (Staleness)
            let live_ok = match dashboard::handlers::build_live_body(&state_snapshot).await {
                Ok(body) => {
                    let mut snap = state_snapshot.metrics_snapshot.write().await;
                    snap.live_body = body;
                    snap.live_built_at = Some(chrono::Utc::now());
                    true
                }
                Err(e) => {
                    tracing::warn!("live metrics query failed, clearing: {e}");
                    let mut snap = state_snapshot.metrics_snapshot.write().await;
                    snap.live_body = String::new();
                    false
                }
            };

            // clickhouse_up: true nur, wenn BEIDE CH-abhaengigen Sektionen in
            // diesem Zyklus gebaut wurden (AND-Kombination, kein Last-Write-Wins).
            {
                let mut snap = state_snapshot.metrics_snapshot.write().await;
                snap.clickhouse_up = Some(latency_ok && live_ok);
            }

            // c) Upstream-Sektion: Provider-/metrics-Endpunkte alle 30s
            // abfragen - bewusst wie ein Prometheus-Scrape (Upstream-Traffic
            // ist Teil der Metrik). Bei PG-Fehler alten Stand behalten:
            // upstream_body bleibt, upstream_built_at wird NICHT aktualisiert
            // (Age-Gauge laeuft an -> sichtbar, statt leeres Body als
            // "0 enabled Provider" zu rendern).
            match dashboard::handlers::build_upstream_body(&state_snapshot).await {
                Ok(body) => {
                    let mut snap = state_snapshot.metrics_snapshot.write().await;
                    snap.upstream_body = body;
                    snap.upstream_built_at = Some(chrono::Utc::now());
                }
                Err(e) => {
                    tracing::warn!("upstream metrics load failed, keeping previous: {e}");
                }
            }
        }
    }));

    // Router bauen: proxy + dashboard
    let app = build_router(state.clone()).merge(dashboard::router::build_router(state));

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
