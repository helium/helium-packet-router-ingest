use crate::uplink::{ingest::UplinkIngest, packet::PacketUp, Gateway, GatewayMac};
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
    GatewayConnect(Gateway),
    GatewayDisconnect(GatewayMac),
    Downlink(GatewayMac, DownlinkRequest),
}

impl MsgSender {
    pub fn new() -> (MsgSender, MsgReceiver) {
        let (tx, rx) = tokio::sync::mpsc::channel(512);
        (MsgSender(tx), rx)
    }

    pub async fn send_downlink(&self, gw_b58: GatewayMac, txpk: DownlinkRequest) {
        self.0
            .send(Msg::Downlink(gw_b58, txpk))
            .await
            .expect("downlink");
    }
}

#[tonic::async_trait]
impl UplinkIngest for MsgSender {
    async fn uplink_receive(&self, packet: PacketUp) {
        self.0.send(Msg::Uplink(packet)).await.expect("uplink");
    }

    async fn gateway_connect(&self, gw: Gateway) {
        self.0
            .send(Msg::GatewayConnect(gw))
            .await
            .expect("gateway connect");
    }

    async fn gateway_disconnect(&self, gw: Gateway) {
        self.0
            .send(Msg::GatewayDisconnect(gw.mac))
            .await
            .expect("gateway disconnect");
    }
}
