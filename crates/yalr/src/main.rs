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

    // Latenz-Snapshot-Task: alle 30s eine 24h-CH-Query, Ergebnis in den
    // Snapshot-Store schreiben. Handle wird wie der Ingest-Handle geboxt und
    // gedroppt (kein graceful Shutdown noetig).
    let state_snapshot = state.clone();
    let _metrics_handle = Box::new(tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            match dashboard::handlers::build_metrics_snapshot_body(&state_snapshot).await {
                Ok(body) => {
                    let mut snap = state_snapshot.metrics_snapshot.write().await;
                    snap.exposition_body = body;
                    snap.clickhouse_up = Some(true);
                    snap.built_at = Some(chrono::Utc::now());
                }
                Err(e) => {
                    tracing::warn!("metrics snapshot query failed, keeping previous: {e}");
                    let mut snap = state_snapshot.metrics_snapshot.write().await;
                    snap.clickhouse_up = Some(false);
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
