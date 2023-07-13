use crate::{actions::MsgSender, downlink};
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
pub fn start(sender: MsgSender, addr: SocketAddr) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let app = Router::new()
            .route("/api/downlink", post(downlink_post))
            .route("/health", get(|| async { "ok" }))
            .layer(Extension(sender));

        tracing::debug!(?addr, "setup");
        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await
            .expect("serve http");
    })
}

async fn downlink_post(
    sender: Extension<MsgSender>,
    extract::Json(downlink): extract::Json<serde_json::Value>,
) -> impl IntoResponse {
    tracing::info!(?downlink, "http downlink");
    match downlink::parse_http_payload(downlink) {
        Ok(resp) => match resp {
            downlink::HttpPayloadResp::Downlink(downlink) => {
                sender.downlink(downlink).await;
                (StatusCode::ACCEPTED, "Downlink Accepted")
            }
            downlink::HttpPayloadResp::Noop => (StatusCode::ACCEPTED, "Answer Accepted"),
        },
        Err(_err) => (StatusCode::BAD_REQUEST, "Unknown"),
    }
}
