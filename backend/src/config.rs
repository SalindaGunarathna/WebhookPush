use std::env;

#[derive(Clone)]
pub struct Config {
    pub bind_addr: String,
    pub public_base_url: String,
    pub db_path: String,
    pub vapid_public_key: String,
    pub vapid_private_key: String,
    pub vapid_subject: String,
    pub max_payload_bytes: usize,
    pub chunk_data_bytes: usize,
    pub chunk_delay_ms: u64,
    pub subscription_ttl_days: i64,
    pub rate_limit_per_minute: u32,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
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
