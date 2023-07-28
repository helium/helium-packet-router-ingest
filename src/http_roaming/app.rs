use super::{
    deduplicator::{Deduplicator, HandlePacket},
    settings::HttpSettings,
    Msg, MsgReceiver, MsgSender,
};
use crate::{
    http_roaming::{downlink::PacketDown, make_pr_start_req, HttpResponse, PRStartReq},
    uplink::{GatewayB58, GatewayTx, PacketHash},
    Result,
};
use axum::http::HeaderMap;
use reqwest::header;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_tracing::TracingMiddleware;
use std::collections::HashMap;
use tracing::Instrument;

pub struct App {
    deduplicator: Deduplicator,
    gateway_map: HashMap<GatewayB58, GatewayTx>,
    pub settings: HttpSettings,
    message_tx: MsgSender,
    message_rx: MsgReceiver,
    client: ClientWithMiddleware,
}

/// After updating app state, these are the side effects that can happen.
#[derive(Debug)]
pub enum UpdateAction {
    Noop,
    StartTimerForNewPacket(PacketHash),
    DownlinkSend(GatewayTx, PacketDown, HttpResponse),
    DownlinkError(HttpResponse),
    UplinkSend(PRStartReq),
}

impl App {
    pub fn new(message_tx: MsgSender, message_rx: MsgReceiver, settings: HttpSettings) -> Self {
        let mut headers_map = HeaderMap::new();
        if let Some(auth_header) = settings.roaming.authorization_header.clone() {
            headers_map.append(
                header::AUTHORIZATION,
                header::HeaderValue::from_str(&auth_header).unwrap(),
            );
        }
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .default_headers(headers_map.clone())
                .build()
                .unwrap(),
        )
        .with(TracingMiddleware::default())
        .build();

        Self {
            deduplicator: Deduplicator::new(),
            gateway_map: HashMap::new(),
            message_tx,
            message_rx,
            settings,
            client,
        }
    }
    pub fn gateway_count(&self) -> usize {
        self.gateway_map.len()
    }

    /// How many packet hashes are stored
    pub fn current_packet_count(&self) -> usize {
        self.deduplicator.packets.len()
    }

    /// How many packet hashes including copies
    pub fn total_current_packet_count(&self) -> usize {
        self.deduplicator.packets.values().map(|x| x.len()).sum()
    }
}

pub async fn start(
    message_tx: MsgSender,
    message_rx: MsgReceiver,
    settings: HttpSettings,
) -> Result {
    let mut app = App::new(message_tx, message_rx, settings);
    loop {
        let action = handle_single_message(&mut app).await;
        handle_update_action(&app, action).await;
    }
}

pub async fn handle_single_message(app: &mut App) -> UpdateAction {
    let message = app.message_rx.recv().await.unwrap();
    handle_message(app, message).await
}

async fn handle_message(app: &mut App, msg: Msg) -> UpdateAction {
    match msg {
        Msg::UplinkReceive(packet) => match app.deduplicator.handle_packet(packet) {
            HandlePacket::New(hash) => UpdateAction::StartTimerForNewPacket(hash),
            HandlePacket::Existing => UpdateAction::Noop,
        },
        Msg::UplinkSend(packet_hash) => match app.deduplicator.get_packets(&packet_hash) {
            None => {
                tracing::warn!(?packet_hash, "message to send unknown packet");
                UpdateAction::Noop
            }
            Some(packets) => {
                tracing::info!(num_packets = packets.len(), "deduplication done");
                match make_pr_start_req(packets, &app.settings.roaming) {
                    Ok(body) => UpdateAction::UplinkSend(body),
                    Err(err) => {
                        tracing::warn!(?packet_hash, ?err, "failed to make pr_start_req");
                        UpdateAction::Noop
                    }
                }
            }
        },
        Msg::UplinkCleanup(packet_hash) => {
            app.deduplicator.remove_packets(&packet_hash);
            UpdateAction::Noop
        }
        Msg::Downlink(packet_down) => match app.gateway_map.get(&packet_down.gateway_b58) {
            None => {
                tracing::warn!(
                    ?packet_down.gateway_b58,
                    "join accept for unknown gateway"
                );
                UpdateAction::DownlinkError(packet_down.http_response.xmit_failed())
            }
            Some(gateway) => {
                let http_response = packet_down.http_response.clone().success();
                UpdateAction::DownlinkSend(gateway.clone(), packet_down, http_response.success())
            }
        },
        Msg::GatewayConnect(gw, sender) => {
            let _prev_val = app.gateway_map.insert(gw, sender);
            tracing::info!(size = app.gateway_map.len(), "gateway connect");
            UpdateAction::Noop
        }
        Msg::GatewayDisconnect(gw) => {
            let _prev_val = app.gateway_map.remove(&gw);
            tracing::info!(size = app.gateway_map.len(), "gateway disconnect");
            UpdateAction::Noop
        }
    }
}

pub async fn handle_update_action(app: &App, action: UpdateAction) {
    match action {
        UpdateAction::Noop => {}

        UpdateAction::StartTimerForNewPacket(hash) => {
            let dedup = app.settings.roaming.dedup_window.into();
            let cleanup = app.settings.cleanup_window.into();
            let sender = app.message_tx.clone();
            let timer_span = tracing::info_span!("packet timers", ?hash, ?dedup, ?cleanup);
            tokio::spawn(
                async move {
                    use tokio::time::sleep;
                    sleep(dedup).await;
                    sender.uplink_send(hash.clone()).await;
                    sleep(cleanup).await;
                    sender.uplink_cleanup(hash).await;
                }
                .instrument(timer_span),
            );
        }
        UpdateAction::DownlinkSend(gw_tx, packet_down, http_response) => {
            gw_tx.send_downlink(packet_down.downlink).await;
            tracing::info!(?packet_down.gateway_b58, "downlink sent");

            if http_response.should_send_for_protocol(&app.settings.roaming.protocol_version) {
                let lns_endpoint = app.settings.lns_endpoint.clone();
                let client = app.client.clone();
                tokio::spawn(async move {
                    let body = serde_json::to_string(&http_response).unwrap();
                    let res = client.post(lns_endpoint).body(body.clone()).send().await;
                    tracing::info!(?body, ?res, "successful downlink post")
                });
            }
        }
        UpdateAction::DownlinkError(http_response) => {
            let lns_endpoint = app.settings.lns_endpoint.clone();
            let client = app.client.clone();
            tokio::spawn(async move {
                let body = serde_json::to_string(&http_response).unwrap();
                let res = client.post(lns_endpoint).body(body.clone()).send().await;
                tracing::info!(?body, ?res, "downlink error post");
            });
        }
        UpdateAction::UplinkSend(pr_start_req) => {
            let lns_endpoint = app.settings.lns_endpoint.clone();
            let client = app.client.clone();
            tokio::spawn(async move {
                let body = serde_json::to_string(&pr_start_req).unwrap();
                let res = client.post(lns_endpoint).body(body.clone()).send().await;
                tracing::info!(?body, ?res, "post");
            });
        }
    }
}
