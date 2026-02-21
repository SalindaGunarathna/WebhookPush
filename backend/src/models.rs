use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Deserialize, Serialize, Clone)]
pub struct PushSubscription {
    pub endpoint: String,
    #[serde(rename = "expirationTime")]
    pub expiration_time: Option<i64>,
    pub keys: PushKeys,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct PushKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StoredSubscription {
    pub subscription: PushSubscription,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct SubscribeResponse {
    pub uuid: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct HookRequest {
    pub id: String,
    pub timestamp: String,
    pub method: String,
    pub path: String,
    pub query_string: String,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub source_ip: String,
    pub content_length: usize,
}

#[derive(Serialize)]
pub struct ChunkEnvelope {
    pub request_id: String,
    pub chunk_index: usize,
    pub total_chunks: usize,
    pub data: String,
}

#[derive(Serialize)]
pub struct ConfigResponse {
    pub public_key: String,
}
