//! Router fuer Dashboard-API + static UI serving.

use axum::routing::{get, post};
use axum::Router;

use crate::handlers;
use common::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        // auth
        .route("/dashboard-api/auth/login", post(handlers::login))
        .route("/dashboard-api/auth/logout", post(handlers::logout))
        .route("/dashboard-api/auth/me", get(handlers::me))
        // keys
        .route("/dashboard-api/keys", get(handlers::list_keys))
        .route("/dashboard-api/keys", post(handlers::create_key))
        .route("/dashboard-api/keys/:id", post(handlers::update_key))
        .route("/dashboard-api/keys/:id", axum::routing::delete(handlers::delete_key))
        .route("/dashboard-api/keys/:id/reveal", post(handlers::reveal_key))
        // providers
        .route("/dashboard-api/providers", get(handlers::list_providers))
        .route("/dashboard-api/providers", post(handlers::create_provider))
        .route("/dashboard-api/providers/discover-metrics", post(handlers::discover_metrics))
        .route("/dashboard-api/providers/:id", axum::routing::delete(handlers::delete_provider))
        .route("/dashboard-api/providers/:id", axum::routing::post(handlers::update_provider))
        .route("/dashboard-api/providers/:id/rename", axum::routing::post(handlers::rename_provider))
        .route("/dashboard-api/providers/:id/refresh-capabilities", axum::routing::post(handlers::refresh_provider_capabilities))
        // models
        .route("/dashboard-api/models", get(handlers::list_models))
        .route("/dashboard-api/models", post(handlers::create_model))
        .route("/dashboard-api/models/:id", axum::routing::put(handlers::update_model))
        .route("/dashboard-api/models/:id", axum::routing::delete(handlers::delete_model))
        // fallbacks
        .route("/dashboard-api/fallbacks", get(handlers::list_fallbacks))
        .route("/dashboard-api/fallbacks", post(handlers::create_fallback))
        .route("/dashboard-api/fallbacks/:id", axum::routing::delete(handlers::delete_fallback))
        // logs & stats
        .route("/dashboard-api/logs", get(handlers::list_logs))
        .route("/dashboard-api/logs/:id", get(handlers::get_log))
        .route("/dashboard-api/live", get(handlers::live_events))
        .route("/dashboard-api/live/stats", get(handlers::live_stats))
        .route("/dashboard-api/stats", get(handlers::stats))
        .route("/dashboard-api/timeseries", get(handlers::timeseries))
        .route("/dashboard-api/breakdown", get(handlers::breakdown))
        // metrics (kein require_session, optionaler Token-Schutz)
        .route("/metrics", get(handlers::metrics_handler))
        // settings
        .route("/dashboard-api/settings/password", post(handlers::change_password))
        .with_state(state);

    let ui = Router::new().merge(crate::static_files::ui_router());

    Router::new().merge(api).merge(ui)
}
