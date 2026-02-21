mod config;
mod db;
mod error;
mod handlers;
mod models;
mod push;
mod rate_limiter;
mod state;

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    routing::{any, delete, get, post},
    Router,
};
use dotenvy::dotenv;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::{
    config::Config,
    db::{cleanup_expired, init_db, open_db},
    handlers::{config as config_handler, hook, index, subscribe, unsubscribe},
    rate_limiter::RateLimiter,
    state::AppState,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cfg = Arc::new(Config::from_env()?);
    let db = Arc::new(open_db(&cfg.db_path).map_err(|err| anyhow::anyhow!(err))?);
    init_db(&db).map_err(|err| anyhow::anyhow!(err))?;
    let rate_limiter = Arc::new(RateLimiter::new(cfg.rate_limit_per_minute));

    let state = AppState {
        db: db.clone(),
        cfg: cfg.clone(),
        rate_limiter,
    };

    if cfg.subscription_ttl_days > 0 {
        let db_clone = db.clone();
        let ttl_days = cfg.subscription_ttl_days;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            loop {
                interval.tick().await;
                if let Err(err) = cleanup_expired(&db_clone, ttl_days) {
                    error!("cleanup failed: {err}");
                }
            }
        });
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(index))
        .route("/api/config", get(config_handler))
        .route("/api/subscribe", post(subscribe))
        .route("/api/subscribe/:uuid", delete(unsubscribe))
        .route("/hook/:uuid", any(hook))
        .route("/:uuid", any(hook))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    info!("listening on {}", cfg.bind_addr);
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}
