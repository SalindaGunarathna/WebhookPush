use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Path, Request, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{any, delete, get, post},
    Json, Router,
};
use base64::{encode as base64_encode, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use dotenvy::dotenv;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::Path as FsPath,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::Mutex, time::sleep};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use web_push::{
    ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushClient, WebPushMessageBuilder,
};

const SUBSCRIPTIONS: TableDefinition<&str, &str> = TableDefinition::new("subscriptions");

#[derive(Clone)]
struct AppState {
    db: Arc<Database>,
    cfg: Arc<Config>,
    rate_limiter: Arc<RateLimiter>,
}

#[derive(Clone)]
struct Config {
    bind_addr: String,
    public_base_url: String,
    db_path: String,
    vapid_public_key: String,
    vapid_private_key: String,
    vapid_subject: String,
    max_payload_bytes: usize,
    chunk_data_bytes: usize,
    chunk_delay_ms: u64,
    subscription_ttl_days: i64,
    rate_limit_per_minute: u32,
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
        let bind_addr = env_or("BIND_ADDR", "0.0.0.0:3000");
        let public_base_url = env_or("PUBLIC_BASE_URL", "http://localhost:3000");
        let db_path = env_or("DB_PATH", "webhookpush.redb");
        let vapid_public_key = env::var("VAPID_PUBLIC_KEY")
            .map_err(|_| anyhow::anyhow!("VAPID_PUBLIC_KEY is required"))?;
        let vapid_private_key = env::var("VAPID_PRIVATE_KEY")
            .map_err(|_| anyhow::anyhow!("VAPID_PRIVATE_KEY is required"))?;
        let vapid_subject = env_or("VAPID_SUBJECT", "mailto:admin@example.com");
        let max_payload_bytes = env_or_parse("MAX_PAYLOAD_BYTES", 100 * 1024)?;
        let chunk_data_bytes = env_or_parse("CHUNK_DATA_BYTES", 2400)?;
        let chunk_delay_ms = env_or_parse("CHUNK_DELAY_MS", 50)?;
        let subscription_ttl_days = env_or_parse("SUBSCRIPTION_TTL_DAYS", 30)?;
        let rate_limit_per_minute = env_or_parse("RATE_LIMIT_PER_MINUTE", 60)?;

        if chunk_data_bytes == 0 {
            return Err(anyhow::anyhow!("CHUNK_DATA_BYTES must be > 0"));
        }
        if max_payload_bytes == 0 {
            return Err(anyhow::anyhow!("MAX_PAYLOAD_BYTES must be > 0"));
        }

        Ok(Self {
            bind_addr,
            public_base_url,
            db_path,
            vapid_public_key,
            vapid_private_key,
            vapid_subject,
            max_payload_bytes,
            chunk_data_bytes,
            chunk_delay_ms,
            subscription_ttl_days,
            rate_limit_per_minute,
        })
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct PushSubscription {
    endpoint: String,
    #[serde(rename = "expirationTime")]
    expiration_time: Option<i64>,
    keys: PushKeys,
}

#[derive(Deserialize, Serialize, Clone)]
struct PushKeys {
    p256dh: String,
    auth: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredSubscription {
    subscription: PushSubscription,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct SubscribeResponse {
    uuid: String,
    url: String,
}

#[derive(Serialize)]
struct HookRequest {
    id: String,
    timestamp: String,
    method: String,
    path: String,
    query_string: String,
    headers: HashMap<String, String>,
    body: String,
    source_ip: String,
    content_length: usize,
}

#[derive(Serialize)]
struct ChunkEnvelope {
    request_id: String,
    chunk_index: usize,
    total_chunks: usize,
    data: String,
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

struct RateLimiter {
    limit_per_minute: u32,
    inner: Mutex<HashMap<String, RateEntry>>,
}

struct RateEntry {
    window_start: Instant,
    count: u32,
}

impl RateLimiter {
    fn new(limit_per_minute: u32) -> Self {
        Self {
            limit_per_minute,
            inner: Mutex::new(HashMap::new()),
        }
    }

    async fn allow(&self, key: &str) -> bool {
        if self.limit_per_minute == 0 {
            return true;
        }

        let mut map = self.inner.lock().await;
        let now = Instant::now();
        let entry = map.entry(key.to_string()).or_insert(RateEntry {
            window_start: now,
            count: 0,
        });

        if now.duration_since(entry.window_start) >= Duration::from_secs(60) {
            entry.window_start = now;
            entry.count = 0;
        }

        if entry.count >= self.limit_per_minute {
            return false;
        }

        entry.count += 1;
        true
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cfg = Arc::new(Config::from_env()?);
    let db = Arc::new(if FsPath::new(&cfg.db_path).exists() {
        Database::open(&cfg.db_path)?
    } else {
        Database::create(&cfg.db_path)?
    });
    let rate_limiter = Arc::new(RateLimiter::new(cfg.rate_limit_per_minute));

    let state = AppState {
        db: db.clone(),
        cfg: cfg.clone(),
        rate_limiter,
    };

    {
        let write_txn = db.begin_write()?;
        write_txn.open_table(SUBSCRIPTIONS)?;
        write_txn.commit()?;
    }

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
        .route("/api/config", get(config))
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

async fn index() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>WebhookPush</title>
  </head>
  <body>
    <h1>WebhookPush</h1>
    <p>Backend running. Frontend will be added in the next step.</p>
  </body>
</html>"#,
    )
}

#[derive(Serialize)]
struct ConfigResponse {
    public_key: String,
}

async fn config(State(state): State<AppState>) -> Json<ConfigResponse> {
    Json(ConfigResponse {
        public_key: state.cfg.vapid_public_key.clone(),
    })
}

async fn subscribe(
    State(state): State<AppState>,
    Json(subscription): Json<PushSubscription>,
) -> Result<Json<SubscribeResponse>, AppError> {
    let uuid = generate_uuid(&state.db)?;
    let stored = StoredSubscription {
        subscription,
        created_at: Utc::now(),
    };
    db_put(&state.db, &uuid, &stored)?;

    let base = state.cfg.public_base_url.trim_end_matches('/');
    let url = format!("{base}/{uuid}");

    Ok(Json(SubscribeResponse { uuid, url }))
}

async fn unsubscribe(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Result<StatusCode, AppError> {
    let removed = db_delete(&state.db, &uuid)?;
    if !removed {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "subscription not found",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn hook(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    req: Request,
) -> Result<StatusCode, AppError> {
    if !state.rate_limiter.allow(&uuid).await {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded",
        ));
    }

    let (parts, body) = req.into_parts();
    let method = parts.method;
    let headers = parts.headers;
    let uri = parts.uri;
    let source_ip = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let body = to_bytes(body, state.cfg.max_payload_bytes + 1)
        .await
        .map_err(|_| AppError::new(StatusCode::PAYLOAD_TOO_LARGE, "payload exceeds limit"))?;

    let stored = match db_get(&state.db, &uuid)? {
        Some(stored) => stored,
        None => {
            return Err(AppError::new(
                StatusCode::NOT_FOUND,
                "subscription not found",
            ));
        }
    };

    let mut headers_map = HashMap::new();
    for (name, value) in headers.iter() {
        let value_str = value.to_str().unwrap_or("<binary>");
        headers_map.insert(name.to_string(), value_str.to_string());
    }

    let request_id = Uuid::new_v4().to_string();
    let payload = HookRequest {
        id: request_id.clone(),
        timestamp: Utc::now().to_rfc3339(),
        method: method.to_string(),
        path: uri.path().to_string(),
        query_string: uri.query().unwrap_or("").to_string(),
        headers: headers_map,
        body: String::from_utf8_lossy(&body).to_string(),
        source_ip,
        content_length: body.len(),
    };

    let payload_bytes = serde_json::to_vec(&payload)?;
    if payload_bytes.len() > state.cfg.max_payload_bytes {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload exceeds limit",
        ));
    }

    let chunks = chunk_bytes(&payload_bytes, state.cfg.chunk_data_bytes);
    let total_chunks = chunks.len();

    for (index, chunk) in chunks.iter().enumerate() {
        let envelope = ChunkEnvelope {
            request_id: request_id.clone(),
            chunk_index: index + 1,
            total_chunks,
            data: base64_encode(chunk),
        };
        let envelope_bytes = serde_json::to_vec(&envelope)?;
        send_push(&state, &stored.subscription, &envelope_bytes).await?;

        if index + 1 < total_chunks {
            sleep(Duration::from_millis(state.cfg.chunk_delay_ms)).await;
        }
    }

    Ok(StatusCode::OK)
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_or_parse<T>(key: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env::var(key) {
        Ok(value) => Ok(value.parse()?),
        Err(_) => Ok(default),
    }
}

fn generate_uuid(db: &Database) -> Result<String, AppError> {
    for _ in 0..5 {
        let candidate = Uuid::new_v4()
            .to_string()
            .replace('-', "")
            .chars()
            .take(8)
            .collect::<String>();
        if db_get(db, &candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    Err(AppError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to allocate unique id",
    ))
}

fn chunk_bytes(bytes: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return vec![Vec::new()];
    }

    bytes
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

fn db_put(db: &Database, uuid: &str, stored: &StoredSubscription) -> Result<(), AppError> {
    let value = serde_json::to_string(stored)?;
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(SUBSCRIPTIONS)?;
        table.insert(uuid, value.as_str())?;
    }
    write_txn.commit()?;
    Ok(())
}

fn db_get(db: &Database, uuid: &str) -> Result<Option<StoredSubscription>, AppError> {
    let read_txn = db.begin_read()?;
    let table = read_txn.open_table(SUBSCRIPTIONS)?;
    if let Some(value) = table.get(uuid)? {
        let stored: StoredSubscription = serde_json::from_str(value.value())?;
        Ok(Some(stored))
    } else {
        Ok(None)
    }
}

fn db_delete(db: &Database, uuid: &str) -> Result<bool, AppError> {
    let write_txn = db.begin_write()?;
    let removed = {
        let mut table = write_txn.open_table(SUBSCRIPTIONS)?;
        table.remove(uuid)?.is_some()
    };
    write_txn.commit()?;
    Ok(removed)
}

fn cleanup_expired(db: &Database, ttl_days: i64) -> Result<(), AppError> {
    let cutoff = Utc::now() - chrono::Duration::days(ttl_days);
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(SUBSCRIPTIONS)?;
        let mut to_remove = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            let stored: StoredSubscription = serde_json::from_str(value.value())?;
            if stored.created_at < cutoff {
                to_remove.push(key.value().to_string());
            }
        }
        for key in to_remove {
            let _ = table.remove(key.as_str());
        }
    }
    write_txn.commit()?;
    Ok(())
}

async fn send_push(
    state: &AppState,
    subscription: &PushSubscription,
    payload: &[u8],
) -> Result<(), AppError> {
    let subscription_info = SubscriptionInfo::new(
        subscription.endpoint.clone(),
        subscription.keys.p256dh.clone(),
        subscription.keys.auth.clone(),
    );

    let mut builder =
        WebPushMessageBuilder::new(&subscription_info).map_err(|err| {
            AppError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("push builder error: {err}"),
            )
        })?;

    builder.set_payload(ContentEncoding::Aes128Gcm, payload);
    builder.set_ttl(60);

    let mut vapid_builder = VapidSignatureBuilder::from_base64(
        &state.cfg.vapid_private_key,
        URL_SAFE_NO_PAD,
        &subscription_info,
    )
    .map_err(|err| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    vapid_builder.add_claim("sub", state.cfg.vapid_subject.as_str());
    let signature = vapid_builder
        .build()
        .map_err(|err| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    builder.set_vapid_signature(signature);

    let message = builder
        .build()
        .map_err(|err| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let client = WebPushClient::new()
        .map_err(|err| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    if let Err(err) = client.send(message).await {
        error!("push failed: {err}");
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            format!("push failed: {err}"),
        ));
    }

    Ok(())
}
