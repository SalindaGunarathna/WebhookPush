use std::sync::Arc;

use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tracing::error;

use crate::{config::Config, error::AppError, models::PushSubscription, push::send_push};
use redb::Database;

#[derive(Clone)]
pub struct PushQueue {
    sender: mpsc::Sender<QueueItem>,
    byte_limiter: Arc<Semaphore>,
}

struct QueueItem {
    uuid: String,
    subscription: PushSubscription,
    payload: Vec<u8>,
    send_after: tokio::time::Instant,
    _bytes_permit: OwnedSemaphorePermit,
}

impl PushQueue {
    pub fn new(
        workers: usize,
        capacity: usize,
        max_bytes: usize,
        db: Arc<Database>,
        cfg: Arc<Config>,
        push_client: web_push::WebPushClient,
    ) -> Self {
        let capacity = capacity.max(1);
        let max_bytes = max_bytes.max(1);
        let workers = workers.max(1);

        let (sender, mut receiver) = mpsc::channel::<QueueItem>(capacity);
        let byte_limiter = Arc::new(Semaphore::new(max_bytes));
        let worker_limiter = Arc::new(Semaphore::new(workers));

        tokio::spawn({
            let worker_limiter = worker_limiter.clone();
            async move {
                while let Some(item) = receiver.recv().await {
                    let permit = match worker_limiter.clone().acquire_owned().await {
                        Ok(permit) => permit,
                        Err(_) => break,
                    };
                    let db = db.clone();
                    let cfg = cfg.clone();
                    let push_client = push_client.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        if item.send_after > tokio::time::Instant::now() {
                            tokio::time::sleep_until(item.send_after).await;
                        }
                        if let Err(err) = send_push(
                            &cfg,
                            &db,
                            &push_client,
                            &item.uuid,
                            &item.subscription,
                            &item.payload,
                        )
                        .await
                        {
                            error!("push failed for {}: {err}", item.uuid);
                        }
                    });
                }
            }
        });

        Self {
            sender,
            byte_limiter,
        }
    }

    pub fn try_enqueue(
        &self,
        uuid: &str,
        subscription: &PushSubscription,
        payload: Vec<u8>,
        send_after: tokio::time::Instant,
    ) -> Result<(), AppError> {
        let bytes = payload.len().max(1);
        let permits = u32::try_from(bytes).unwrap_or(u32::MAX);

        let bytes_permit = self
            .byte_limiter
            .clone()
            .try_acquire_many_owned(permits)
            .map_err(|_| {
                AppError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "queue full",
                )
            })?;

        let item = QueueItem {
            uuid: uuid.to_string(),
            subscription: subscription.clone(),
            payload,
            send_after,
            _bytes_permit: bytes_permit,
        };

        match self.sender.try_send(item) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = err.into_inner();
                Err(AppError::new(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "queue full",
                ))
            }
        }
    }
}
