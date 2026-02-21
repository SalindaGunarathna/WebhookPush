use axum::http::StatusCode;
use base64::URL_SAFE_NO_PAD;
use tracing::error;
use web_push::{
    ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushClient, WebPushMessageBuilder,
};

use crate::{error::AppError, models::PushSubscription, state::AppState};

pub async fn send_push(
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
