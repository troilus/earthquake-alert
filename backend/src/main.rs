mod config;
mod db;
mod models;
mod routes;
mod services;
mod utils;

use anyhow::Result;
use axum::{
    Router,
    routing::{delete, get, post},
};
use config::Config;
use db::Database;
use models::CachedEarthquake;
use routes::{
    AppState, get_subscription_handler, health_handler, stats_handler, subscribe_handler,
    test_earthquake_handler, test_notify_handler, unsubscribe_by_path_handler,
};
use services::EarthquakeMonitor;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "earthquake_alert_backend=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 加载配置
    let config = Config::from_env();
    tracing::info!("配置加载完成: {:?}", config);

    // 打开数据库
    let db = Database::open(&config.db_path)?;
    tracing::info!("数据库已打开: {}", config.db_path);

    // 创建缓存和状态
    let latest_earthquake: Arc<Mutex<Option<CachedEarthquake>>> =
        Arc::new(Mutex::new(None));
    let state = AppState {
        db: db.clone(),
        latest_earthquake: latest_earthquake.clone(),
    };

    // 创建路由
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/api/subscribe", post(subscribe_handler))
        .route(
            "/api/unsubscribe/{bark_id}",
            delete(unsubscribe_by_path_handler),
        )
        .route("/api/stats", get(stats_handler))
        .route(
            "/api/subscription/{bark_id}",
            get(get_subscription_handler),
        )
        .route("/api/test-earthquake", get(test_earthquake_handler))
        .route("/api/test-notify", post(test_notify_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .fallback(get(index_handler))
        .with_state(state);

    // 启动服务器
    let addr: SocketAddr = format!("{}:{}", config.server_host, config.server_port).parse()?;

    tracing::info!("服务器启动中: http://{}", addr);

    // 后台缓存任务：每 5 分钟拉取一次最新地震数据
    let cache = latest_earthquake.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1800));
        // 启动后立即拉取一次
        interval.tick().await;
        loop {
            match client.get("https://api.wolfx.jp/cenc_eew.json").send().await {
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(val) => {
                        let eq = CachedEarthquake {
                            event_id: val
                                .get("EventID")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            origin_time: val
                                .get("OriginTime")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            hypocenter: val
                                .get("HypoCenter")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            latitude: val
                                .get("Latitude")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0),
                            longitude: val
                                .get("Longitude")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0),
                            magnitude: val
                                .get("Magnitude")
                                .or_else(|| val.get("Magunitude"))
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0),
                            depth: val
                                .get("Depth")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0),
                            max_intensity: val
                                .get("MaxIntensity")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0),
                        };
                        *cache.lock().unwrap() = Some(eq);
                        tracing::info!("地震缓存已更新");
                    }
                    Err(e) => tracing::warn!("解析地震缓存失败: {:?}", e),
                },
                Err(e) => tracing::warn!("获取地震缓存失败: {:?}", e),
            }
            interval.tick().await;
        }
    });

    // 在后台任务中启动地震监控（支持百万级并发）
    let monitor = EarthquakeMonitor::new(
        db,
        config.bark_api_url.clone(),
        config.eew_websocket_url.clone(),
        config.http_pool_size,
        config.max_concurrent_notifications,
        config.batch_size,
    );
    tokio::spawn(async move {
        if let Err(e) = monitor.start().await {
            tracing::error!("地震监控服务错误: {:?}", e);
        }
    });

    // 启动 HTTP 服务器
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("前端页面: http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn index_handler() -> impl axum::response::IntoResponse {
    axum::response::Html(include_str!("../static/index.html"))
}
