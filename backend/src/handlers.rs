use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Path, Request, State},
    http::StatusCode,
    response::Html,
    Json,
};
use base64::encode as base64_encode;
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

pub async fn unsubscribe(
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

pub async fn hook(
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

fn chunk_bytes(bytes: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    if bytes.is_empty() {
        return vec![Vec::new()];
    }

    bytes
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}
