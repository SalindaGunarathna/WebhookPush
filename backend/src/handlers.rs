use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    Json,
};
use base64::{decode_config, encode as base64_encode, URL_SAFE, URL_SAFE_NO_PAD};
use chrono::Utc;
use std::{
    collections::HashMap,
    net::SocketAddr,
    time::Duration,
};
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    db::{db_delete, db_get, db_put, generate_uuid},
    error::AppError,
    models::{
        ChunkEnvelope, ConfigResponse, HookRequest, PushSubscription, StoredSubscription,
        SubscribeResponse,
    },
    push::send_push,
    state::AppState,
};

pub async fn index() -> Html<&'static str> {
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

pub async fn config(State(state): State<AppState>) -> Json<ConfigResponse> {
    Json(ConfigResponse {
        public_key: state.cfg.vapid_public_key.clone(),
    })
}

pub async fn subscribe(
    State(state): State<AppState>,
    Json(subscription): Json<PushSubscription>,
) -> Result<Json<SubscribeResponse>, AppError> {
    validate_subscription(&subscription)?;

    let uuid = generate_uuid(&state.db)?;
    let delete_token = Uuid::new_v4().to_string().replace('-', "");
    let stored = StoredSubscription {
        subscription,
        created_at: Utc::now(),
        delete_token: delete_token.clone(),
    };
    db_put(&state.db, &uuid, &stored)?;

    let base = state.cfg.public_base_url.trim_end_matches('/');
    let url = format!("{base}/{uuid}");

    Ok(Json(SubscribeResponse {
        uuid,
        url,
        delete_token,
    }))
}

pub async fn unsubscribe(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, AppError> {
    let provided = headers
        .get("x-delete-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() {
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "delete token required",
        ));
    }

    let stored = match db_get(&state.db, &uuid)? {
        Some(stored) => stored,
        None => {
            return Err(AppError::new(
                StatusCode::NOT_FOUND,
                "subscription not found",
            ));
        }
    };

    if stored.delete_token != provided {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "invalid delete token",
        ));
    }

    let _ = db_delete(&state.db, &uuid)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn hook(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    req: Request,
) -> Result<StatusCode, AppError> {
    let (parts, body) = req.into_parts();
    let method = parts.method;
    let headers = parts.headers;
    let uri = parts.uri;
    let source_ip = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let stored = match db_get(&state.db, &uuid)? {
        Some(stored) => stored,
        None => {
            return Err(AppError::new(
                StatusCode::NOT_FOUND,
                "subscription not found",
            ));
        }
    };

    if !state.rate_limiter.allow(&uuid).await {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded",
        ));
    }

    let body = to_bytes(body, state.cfg.max_payload_bytes + 1)
        .await
        .map_err(|_| AppError::new(StatusCode::PAYLOAD_TOO_LARGE, "payload exceeds limit"))?;

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

    let (chunk_size, total_chunks) =
        resolve_chunking(&payload_bytes, &request_id, state.cfg.chunk_data_bytes)?;
    let chunks = chunk_bytes(&payload_bytes, chunk_size);

    for (index, chunk) in chunks.iter().enumerate() {
        let envelope = ChunkEnvelope {
            request_id: request_id.clone(),
            chunk_index: index + 1,
            total_chunks,
            data: base64_encode(chunk),
        };
        let envelope_bytes = serde_json::to_vec(&envelope)?;
        send_push(&state, &uuid, &stored.subscription, &envelope_bytes).await?;

        if index + 1 < total_chunks {
            sleep(Duration::from_millis(state.cfg.chunk_delay_ms)).await;
        }
    }

    Ok(StatusCode::OK)
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

fn validate_subscription(subscription: &PushSubscription) -> Result<(), AppError> {
    let endpoint = subscription.endpoint.trim();
    if endpoint.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "endpoint required"));
    }
    if endpoint.len() > 2048 {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "endpoint too long"));
    }
    if !endpoint.starts_with("https://") {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "endpoint must be https",
        ));
    }

    if subscription.keys.p256dh.len() > 256 || subscription.keys.auth.len() > 128 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "subscription keys too long",
        ));
    }

    let p256dh_bytes = decode_b64url(&subscription.keys.p256dh)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "invalid p256dh"))?;
    if p256dh_bytes.len() != 65 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid p256dh length",
        ));
    }

    let auth_bytes = decode_b64url(&subscription.keys.auth)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "invalid auth"))?;
    if auth_bytes.len() != 16 {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "invalid auth length",
        ));
    }

    Ok(())
}

fn decode_b64url(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    decode_config(value, URL_SAFE_NO_PAD).or_else(|_| decode_config(value, URL_SAFE))
}

fn envelope_overhead_bytes(
    request_id: &str,
    chunk_index: usize,
    total_chunks: usize,
) -> Result<usize, AppError> {
    let envelope = ChunkEnvelope {
        request_id: request_id.to_string(),
        chunk_index,
        total_chunks,
        data: String::new(),
    };
    Ok(serde_json::to_vec(&envelope)?.len())
}

fn resolve_chunking(
    payload: &[u8],
    request_id: &str,
    configured: usize,
) -> Result<(usize, usize), AppError> {
    let mut chunk_size =
        max_chunk_data_bytes(configured, envelope_overhead_bytes(request_id, 1, 1)?)?;
    let mut total_chunks = (payload.len() + chunk_size - 1) / chunk_size;

    loop {
        let overhead = envelope_overhead_bytes(request_id, total_chunks, total_chunks)?;
        let next_chunk_size = max_chunk_data_bytes(configured, overhead)?;
        let next_total_chunks = (payload.len() + next_chunk_size - 1) / next_chunk_size;

        if next_chunk_size == chunk_size && next_total_chunks == total_chunks {
            break;
        }

        chunk_size = next_chunk_size;
        total_chunks = next_total_chunks;
    }

    Ok((chunk_size, total_chunks))
}

fn max_chunk_data_bytes(configured: usize, overhead: usize) -> Result<usize, AppError> {
    const MAX_ENVELOPE_BYTES: usize = 3300;
    if overhead >= MAX_ENVELOPE_BYTES {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk overhead exceeds push limit",
        ));
    }

    let available = MAX_ENVELOPE_BYTES - overhead;
    let mut max_raw = (available / 4) * 3;
    while 4 * ((max_raw + 2) / 3) > available {
        max_raw = max_raw.saturating_sub(1);
    }

    let chunk_size = configured.min(max_raw);
    if chunk_size == 0 {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "chunk size too small",
        ));
    }

    Ok(chunk_size)
}
