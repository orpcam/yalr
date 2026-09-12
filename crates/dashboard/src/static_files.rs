//! Serving der gebauten React-UI (dashboard-ui/dist), embedded zur compile-zeit.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use include_dir::{include_dir, Dir};

static UI_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../dashboard-ui/dist");

pub fn ui_router() -> Router {
    // include_dir! schlaegt bereits fehl, wenn dist/ fehlt. Falls dist/ aber
    // existiert und leer/unvollstaendig ist (z.b. abgebrochener UI-build),
    // hier ein klarer fehler statt eines binarys, das nur 404 serven wuerde.
    assert!(
        UI_DIR.get_file("index.html").is_some(),
        "dashboard-ui/dist/index.html fehlt - erst das React-UI bauen: cd dashboard-ui && npm ci && npm run build"
    );
    Router::new().route("/", get(index)).route("/*path", get(static_handler))
}

async fn index() -> Response {
    serve_file("index.html")
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        return serve_file("index.html");
    }
    match serve_file(path) {
        r if r.status() == StatusCode::NOT_FOUND => serve_file("index.html"), // SPA-fallback
        r => r,
    }
}

fn serve_file(path: &str) -> Response {
    // pfad-traversal verhindern
    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    let Some(file) = UI_DIR.get_file(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let content = file.contents();
    let mime = mime_for(path);
    // vite hasht nur die asset-dateinamen: /assets/* ist dadurch inhaltlich
    // unveraenderlich und darf lange gecacht werden. index.html (und alles
    // unmeshashte) muss immer neu validiert werden, sonst dient der browser
    // nach einem deploy eine alte html aus, die auf nicht mehr existierende
    // assets zeigt.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)],
        content,
    )
        .into_response()
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
