use axum::{
    extract::Path,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};

/// Minified at build time by `build.rs` into `OUT_DIR/index.min.html`.
const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.min.html"));

pub async fn index_handler() -> impl IntoResponse {
    Html(INDEX_HTML)
}

pub async fn tutorial_image_handler(Path(filename): Path<String>) -> Response {
    let bytes: Option<&'static [u8]> = match filename.as_str() {
        "bark.1.1.png" => Some(include_bytes!("../../web/img/bark.1.1.png")),
        "bark.1.2.png" => Some(include_bytes!("../../web/img/bark.1.2.png")),
        "bark.1.3.png" => Some(include_bytes!("../../web/img/bark.1.3.png")),
        "bark.2.1.png" => Some(include_bytes!("../../web/img/bark.2.1.png")),
        "bark.2.2.png" => Some(include_bytes!("../../web/img/bark.2.2.png")),
        "bark.2.3.png" => Some(include_bytes!("../../web/img/bark.2.3.png")),
        _ => None,
    };
    match bytes {
        Some(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
