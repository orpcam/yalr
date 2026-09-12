//! Axum-Router fuer das Gateway.

use axum::routing::{get, post};
use axum::Router;

use crate::proxy;
use common::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let proxy_routes = Router::new()
        .route("/v1/chat/completions", post(proxy::proxy))
        .route("/v1/embeddings", post(proxy::proxy))
        .route("/v1/messages", post(proxy::proxy))
        .route("/v1/models", get(proxy::models_list));

    Router::new()
        .merge(proxy_routes)
        .route("/health", get(health))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}
