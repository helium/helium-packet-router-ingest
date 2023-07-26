use super::{settings::RoamingSettings, MsgSender};
use crate::{http_roaming::downlink, Result};
use axum::{
    extract,
    response::IntoResponse,
    routing::{get, post, Router},
    Extension,
};
use reqwest::StatusCode;
use std::net::SocketAddr;
use tracing::instrument;

#[instrument]
pub async fn start(sender: MsgSender, addr: SocketAddr, settings: RoamingSettings) -> Result {
    let app = Router::new()
        .route("/api/downlink", post(downlink_post))
        .route("/health", get(|| async { "ok" }))
        .layer(Extension((sender, settings)));

    tracing::debug!(?addr, "setup");
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .map_err(anyhow::Error::from)
}

async fn downlink_post(
    Extension((sender, settings)): Extension<(MsgSender, RoamingSettings)>,
    extract::Json(downlink): extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    tracing::info!(?downlink, "http downlink");
    match downlink::parse_http_payload(downlink, &settings) {
        Ok(resp) => match resp {
            Some(packet_down) => {
                sender.downlink(packet_down).await;
                (StatusCode::ACCEPTED, "Downlink Accepted")
            }
            None => (StatusCode::ACCEPTED, "Answer Accepted"),
        },
        Err(_err) => (StatusCode::BAD_REQUEST, "Unknown"),
    }
}
