//! Umgebungsbasierte Konfiguration.

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,

    pub admin_username: String,
    pub admin_password: String,
    pub session_secret: String,

    pub postgres_url: String,
    pub clickhouse_url: String,
    pub clickhouse_db: String,
    pub clickhouse_user: String,
    pub clickhouse_password: String,

    /// Secret zum Verschluesseln der Provider-API-Keys (AES-ähnlich via XOR+base64 ist
    /// bewusst simpel; für production ggf. auf eine echte AEAD-Library umstellen).
    pub encryption_key: String,

    /// Max. Laenge von request/response body im Log (Bytes, Rest wird
    /// abgeschnitten und als truncated markiert).
    pub log_body_max_len: usize,
}

fn var_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let pg_host = var_or("POSTGRES_HOST", "localhost");
        let pg_port = var_or("POSTGRES_PORT", "5432");
        let pg_user = var_or("POSTGRES_USER", "yalr");
        let pg_password = var_or("POSTGRES_PASSWORD", "yalr-secret");
        let pg_db = var_or("POSTGRES_DB", "yalr");
        let postgres_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            format!("postgres://{pg_user}:{pg_password}@{pg_host}:{pg_port}/{pg_db}")
        });

        let ch_host = var_or("CLICKHOUSE_HOST", "localhost");
        let ch_port = var_or("CLICKHOUSE_PORT", "8123");
        let clickhouse_url =
            env::var("CLICKHOUSE_URL").unwrap_or_else(|_| format!("http://{ch_host}:{ch_port}"));

        Ok(Self {
            host: var_or("HOST", "0.0.0.0"),
            port: var_or("PORT", "8080").parse()?,
            admin_username: var_or("ADMIN_USERNAME", "admin"),
            admin_password: var_or("ADMIN_PASSWORD", "change-me-please"),
            session_secret: var_or("SESSION_SECRET", "insecure-dev-secret-change-me"),
            postgres_url,
            clickhouse_url,
            clickhouse_db: var_or("CLICKHOUSE_DB", "yalr"),
            clickhouse_user: var_or("CLICKHOUSE_USER", "default"),
            clickhouse_password: var_or("CLICKHOUSE_PASSWORD", ""),
            encryption_key: var_or("SESSION_SECRET", "insecure-dev-secret-change-me"),
            log_body_max_len: env::var("LOG_BODY_MAX_LEN")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(32 * 1024),
        })
    }
}
