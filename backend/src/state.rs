use std::sync::Arc;

use redb::Database;

use crate::{config::Config, rate_limiter::RateLimiter};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub cfg: Arc<Config>,
    pub rate_limiter: Arc<RateLimiter>,
}
