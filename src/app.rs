use crate::{
    deduplicator::{Deduplicator, HandlePacket},
    downlink::PacketDown,
    settings::Settings,
    uplink,
    uplink::{GatewayB58, PacketHash, PacketUp},
    uplink_ingest::GatewayTx,
};
use std::collections::HashMap;
use tokio::sync::mpsc::{Receiver, Sender};

pub struct App {
    deduplicator: Deduplicator,
    gateway_map: HashMap<GatewayB58, GatewayTx>,
    settings: Settings,
    message_tx: MsgSender,
    message_rx: Receiver<Msg>,
}

#[derive(Debug, Clone)]
pub struct MsgSender(pub Sender<Msg>);
pub type MsgReceiver = Receiver<Msg>;

/// This message contains the entirety of the lifecycle of routing packets from hpr through http.
///
/// At a gateway's first packet, we register a downlink handler for a gateway.
/// Once a gateway is known packets will be ingested normally.
///
/// Packets are sent to the deduplicator, which will group packets by hash.
/// A `dedup_window` timer is started, after which the packets are forwarded to the roamer.
/// After, a `cleanup_window` timer is started to prevent immediate double delivery of late packets.
///   Setting this too short may result in double delivery of packet groups.
///   Too long may lead to memory issues.
#[derive(Debug)]
pub enum Msg {
    /// Incoming Packet from a Gateway.
    UplinkReceive(PacketUp),
    /// Deduplication timer has elapsed.
    UplinkSend(PacketHash),
    /// Cleanup timer has elapsed.
    UplinkCleanup(PacketHash),
    /// Downlink received for gateway from HTTP handler.
    Downlink(PacketDown),
    /// Gateway has Connected.
    GatewayConnect(GatewayB58, GatewayTx),
    /// Gateway has Disconnected.
    GatewayDisconnect(GatewayB58),
}

impl MsgSender {
    pub fn new() -> (MsgSender, MsgReceiver) {
        let (tx, rx) = tokio::sync::mpsc::channel(512);
        (MsgSender(tx), rx)
    }

    pub async fn uplink_receive(&self, packet: PacketUp) {
        self.0
            .send(Msg::UplinkReceive(packet))
            .await
            .expect("uplink");
    }

    pub async fn uplink_send(&self, key: PacketHash) {
        self.0.send(Msg::UplinkSend(key)).await.expect("dedup done");
    }

    pub async fn uplink_cleanup(&self, key: PacketHash) {
        self.0
            .send(Msg::UplinkCleanup(key))
            .await
            .expect("dedup_cleanup");
    }

    pub async fn gateway_connect(&self, gateway_b58: GatewayB58, downlink_sender: GatewayTx) {
        self.0
            .send(Msg::GatewayConnect(gateway_b58, downlink_sender))
            .await
            .expect("gateway_connect");
    }

    pub async fn gateway_disconnect(&self, gateway: GatewayB58) {
        self.0
            .send(Msg::GatewayDisconnect(gateway))
            .await
            .expect("gateway_disconnect");
    }

    pub async fn downlink(&self, downlink: PacketDown) {
        self.0
            .send(Msg::Downlink(downlink))
            .await
            .expect("downlink");
    }
}

/// After updating app state, these are the side effects that can happen.
#[derive(Debug)]
pub enum UpdateAction {
    Noop,
    StartTimerForNewPacket(PacketHash),
    SendDownlink(GatewayTx, PacketDown),
    SendUplink(String),
}

impl App {
    pub fn new(message_tx: MsgSender, message_rx: Receiver<Msg>, settings: Settings) -> Self {
        Self {
            deduplicator: Deduplicator::new(),
            gateway_map: HashMap::new(),
            message_tx,
            message_rx,
            settings,
        }
    }
    pub fn gateway_count(&self) -> usize {
        self.gateway_map.len()
    }
}

pub fn start(
    message_tx: MsgSender,
    message_rx: Receiver<Msg>,
    settings: Settings,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut app = App::new(message_tx, message_rx, settings);
        loop {
            let action = handle_single_message(&mut app).await;
            handle_update_action(&app, action).await;
        }
    })
}

pub async fn handle_single_message(app: &mut App) -> UpdateAction {
    let message = app.message_rx.recv().await.unwrap();
    update(app, message).await
}

pub async fn handle_update_action(app: &App, action: UpdateAction) {
    match action {
        UpdateAction::Noop => {}
        UpdateAction::StartTimerForNewPacket(hash) => {
            let dedup = app.settings.roaming.dedup_window.into();
            let cleanup = app.settings.cleanup_window.into();
            let sender = app.message_tx.clone();
            tokio::spawn(async move {
                use tokio::time::sleep;
                sleep(dedup).await;
                sender.uplink_send(hash.clone()).await;
                sleep(cleanup).await;
                sender.uplink_cleanup(hash).await;
            });
        }
        UpdateAction::SendDownlink(gw, downlink) => {
            let gateway_name = downlink.gateway();
            gw.send_downlink(downlink.to_packet_down()).await;
            tracing::info!(gw = gateway_name, "downlink sent");

            if let Some(body) = downlink.http_body(&app.settings.roaming) {
                let res = reqwest::Client::new()
                    .post(app.settings.network.lns_endpoint.clone())
                    .body(body.clone())
                    .send()
                    .await;
                tracing::info!(?body, ?res, "post");
            }
        }
        UpdateAction::SendUplink(body) => {
            let res = reqwest::Client::new()
                .post(app.settings.network.lns_endpoint.clone())
                .body(body.clone())
                .send()
                .await;
            tracing::info!(?body, ?res, "post");
        }
    }
}

async fn update(app: &mut App, msg: Msg) -> UpdateAction {
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
                match uplink::make_pr_start_req(packets, &app.settings.roaming) {
                    Ok(body) => UpdateAction::SendUplink(body),
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
        Msg::Downlink(source) => {
            let gw = source.gateway();
            match app.gateway_map.get(&gw) {
                None => {
                    tracing::warn!(?gw, "join accept for unknown gateway");
                    UpdateAction::Noop
                }
                Some(gateway) => UpdateAction::SendDownlink(gateway.clone(), source),
            }
        }
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
