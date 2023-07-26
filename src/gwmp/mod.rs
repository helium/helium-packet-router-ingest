use crate::uplink::{
    ingest::{GatewayID, UplinkIngest},
    packet::PacketUp,
};
use semtech_udp::client_runtime::DownlinkRequest;
use tokio::sync::mpsc::{Receiver, Sender};

pub mod app;
pub mod packet;
pub mod settings;

#[derive(Debug, Clone)]
pub struct MsgSender(pub Sender<Msg>);
pub type MsgReceiver = Receiver<Msg>;

#[derive(Debug)]
pub enum Msg {
    Uplink(PacketUp),
    GatewayConnect(GatewayID),
    GatewayDisconnect(String),
    Downlink(String, DownlinkRequest),
}

impl MsgSender {
    pub fn new() -> (MsgSender, MsgReceiver) {
        let (tx, rx) = tokio::sync::mpsc::channel(512);
        (MsgSender(tx), rx)
    }

    pub async fn send_downlink(&self, gw_id: String, txpk: DownlinkRequest) {
        self.0
            .send(Msg::Downlink(gw_id, txpk))
            .await
            .expect("downlink");
    }
}

#[tonic::async_trait]
impl UplinkIngest for MsgSender {
    async fn uplink_receive(&self, packet: PacketUp) {
        self.0.send(Msg::Uplink(packet)).await.expect("uplink");
    }

    async fn gateway_connect(&self, gw: GatewayID) {
        self.0
            .send(Msg::GatewayConnect(gw))
            .await
            .expect("gateway connect");
    }

    async fn gateway_disconnect(&self, gateway_b58: String) {
        self.0
            .send(Msg::GatewayDisconnect(gateway_b58))
            .await
            .expect("gateway disconnect");
    }
}
