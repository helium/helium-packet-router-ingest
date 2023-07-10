use crate::{
    actions::MsgSender,
    roaming::{PRStartAnsDownlink, PRStartAnsPlain, XmitDataReq},
};
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
    match serde_json::from_value::<PRStartAnsDownlink>(downlink.clone()) {
        Ok(pr_start) => {
            sender.downlink(pr_start).await;
            (StatusCode::ACCEPTED, "Join Accept")
        }
        Err(_) => match serde_json::from_value::<PRStartAnsPlain>(downlink.clone()) {
            Ok(_plain) => (StatusCode::ACCEPTED, "Answer Accepted"),
            Err(_) => match serde_json::from_value::<XmitDataReq>(downlink) {
                Ok(xmit) => {
                    sender.downlink(xmit).await;
                    (StatusCode::ACCEPTED, "XmitReq")
                }
                Err(_) => {
                    tracing::error!("could not make pr_start_ans or xmit_data_req");
                    (StatusCode::BAD_REQUEST, "Nnknown")
                }
            },
        },
    }
}
