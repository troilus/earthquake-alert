use axum::{response::Html, response::IntoResponse};

/// Minified at build time by `build.rs` into `OUT_DIR/index.min.html`.
const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.min.html"));

pub async fn index_handler() -> impl IntoResponse {
    Html(INDEX_HTML)
}
