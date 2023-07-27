use tokio::sync::mpsc::{Receiver, Sender};

use self::{
    downlink::PacketDown,
    settings::{ProtocolVersion, RoamingSettings},
    ul_token::{make_data_token, make_join_token, Token},
};
use crate::{
    region,
    uplink::{
        ingest::UplinkIngest,
        packet::{PacketUp, RoutingInfo},
        DevAddr, Eui, Gateway, GatewayB58, GatewayMac, GatewayTx, PacketHash,
    },
    Result,
};

pub mod app;
pub mod deduplicator;
pub mod downlink;
pub mod downlink_ingest;
pub mod settings;
pub mod ul_token;

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

#[tonic::async_trait]
impl UplinkIngest for MsgSender {
    async fn uplink_receive(&self, packet: PacketUp) {
        metrics::increment_counter!("uplink_receive");
        self.0
            .send(Msg::UplinkReceive(packet))
            .await
            .expect("uplink");
    }
    async fn gateway_connect(&self, gw: Gateway) {
        metrics::increment_gauge!("connected_gateways", 1.0);
        self.0
            .send(Msg::GatewayConnect(gw.b58, gw.tx))
            .await
            .expect("gateway_connect");
    }

    async fn gateway_disconnect(&self, gateway: Gateway) {
        metrics::decrement_gauge!("connected_gateways", 1.0);
        self.0
            .send(Msg::GatewayDisconnect(gateway.b58))
            .await
            .expect("gateway_disconnect");
    }
}

impl MsgSender {
    pub fn new() -> (MsgSender, MsgReceiver) {
        let (tx, rx) = tokio::sync::mpsc::channel(512);
        (MsgSender(tx), rx)
    }

    pub async fn uplink_send(&self, key: PacketHash) {
        metrics::increment_counter!("uplink_send");
        self.0.send(Msg::UplinkSend(key)).await.expect("dedup done");
    }

    pub async fn uplink_cleanup(&self, key: PacketHash) {
        metrics::increment_counter!("uplink cleanup");
        self.0
            .send(Msg::UplinkCleanup(key))
            .await
            .expect("dedup_cleanup");
    }

    pub async fn downlink(&self, downlink: PacketDown) {
        metrics::increment_counter!("downlink");
        self.0
            .send(Msg::Downlink(downlink))
            .await
            .expect("downlink");
    }
}

/// Uplinks
pub fn make_pr_start_req(packets: &[PacketUp], config: &RoamingSettings) -> Result<PRStartReq> {
    let packet = packets.first().expect("at least one packet");

    let (devaddr, dev_eui, token) = match packet.routing_info() {
        RoutingInfo::Eui { dev, .. } => (
            None,
            Some(dev),
            make_join_token(packet.gateway_b58(), packet.timestamp(), packet.region()),
        ),
        RoutingInfo::DevAddr(devaddr) => (
            Some(devaddr),
            None,
            make_data_token(packet.gateway_b58(), packet.timestamp(), packet.region()),
        ),
        RoutingInfo::Unknown => anyhow::bail!("packet contains unparseable routing information"),
    };

    let mut gw_info = vec![];
    for packet in packets.iter() {
        gw_info.push(GWInfo {
            id: packet.gateway_mac(),
            region: packet.region(),
            rssi: packet.rssi(),
            snr: packet.snr(),
            dl_allowed: true,
        });
    }

    Ok(PRStartReq {
        protocol_version: "1.1".to_string(),
        sender_nsid: config.sender_nsid.to_owned(),
        receiver_nsid: config.receiver_nsid.to_owned(),
        dedup_window_size: config.dedup_window.to_string(),
        sender_id: config.helium_net_id.to_owned(),
        receiver_id: config.target_net_id.to_owned(),
        phy_payload: packet.json_payload(),
        ul_meta_data: ULMetaData {
            devaddr,
            dev_eui,
            data_rate: packet.datarate_index(),
            ul_freq: packet.frequency_mhz(),
            recv_time: packet.recv_time(),
            rf_region: packet.region(),
            fns_ul_token: token,
            gw_cnt: gw_info.len(),
            gw_info,
        },
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "MessageType")]
pub struct PRStartReq {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderNSID")]
    pub sender_nsid: String,
    #[serde(rename = "ReceiverNSID")]
    pub receiver_nsid: String,
    #[serde(rename = "DedupWindowSize")]
    pub dedup_window_size: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "PHYPayload")]
    pub phy_payload: String,
    #[serde(rename = "ULMetaData")]
    pub ul_meta_data: ULMetaData,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ULMetaData {
    #[serde(rename = "DevAddr")]
    pub devaddr: Option<DevAddr>,
    #[serde(rename = "DevEUI")]
    pub dev_eui: Option<Eui>,
    #[serde(rename = "DataRate")]
    pub data_rate: region::DR,
    #[serde(rename = "ULFreq")]
    pub ul_freq: f64,
    #[serde(rename = "RecvTime")]
    pub recv_time: String,
    #[serde(rename = "RFRegion")]
    pub rf_region: region::Region,
    #[serde(rename = "FNSULToken")]
    pub fns_ul_token: String,
    #[serde(rename = "GWCnt")]
    pub gw_cnt: usize,
    #[serde(rename = "GWInfo")]
    pub gw_info: Vec<GWInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GWInfo {
    #[serde(rename = "ID")]
    pub id: GatewayMac,
    #[serde(rename = "RFRegion")]
    pub region: region::Region,
    #[serde(rename = "RSSI")]
    pub rssi: i32,
    #[serde(rename = "SNR")]
    pub snr: f32,
    #[serde(rename = "DLAllowed")]
    pub dl_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HttpResponse {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "MessageType")]
    pub message_type: HttpResponseMessageType,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "SenderNSID")]
    pub sender_nsid: String,
    #[serde(rename = "ReceiverNSID")]
    pub receiver_nsid: String,
    #[serde(rename = "Result")]
    pub result: HttpResponseResult,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum HttpResponseMessageType {
    PRStartNotif,
    XmitDataAns,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "ResultCode")]
pub enum HttpResponseResult {
    Success,
    MICFailed,
    XmitFailed,
}

impl HttpResponse {
    pub fn success(mut self) -> Self {
        self.result = HttpResponseResult::Success;
        self
    }

    pub fn mic_failed(mut self) -> Self {
        self.result = HttpResponseResult::MICFailed;
        self
    }

    pub fn xmit_failed(mut self) -> Self {
        self.result = HttpResponseResult::XmitFailed;
        self
    }

    pub fn should_send_for_protocol(&self, protocol_version: &ProtocolVersion) -> bool {
        match (&self.message_type, protocol_version) {
            (HttpResponseMessageType::PRStartNotif, ProtocolVersion::V1_0) => false,
            (HttpResponseMessageType::PRStartNotif, ProtocolVersion::V1_1) => true,
            (HttpResponseMessageType::XmitDataAns, ProtocolVersion::V1_0) => true,
            (HttpResponseMessageType::XmitDataAns, ProtocolVersion::V1_1) => true,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct XmitDataReq {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "PHYPayload")]
    pub phy_payload: String,
    #[serde(rename = "DLMetaData")]
    pub dl_meta_data: DLMetaData,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAns {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Result")]
    pub result: PRStartAnsResult,
    #[serde(rename = "PHYPayload")]
    pub phy_payload: String,
    #[serde(rename = "DLMetaData")]
    pub dl_meta_data: DLMetaData,
}

/// This type exists to parse a PRStartAns that contains no downlink,
/// rather than making DLMetaData optional.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAnsPlain {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Result")]
    pub result: PRStartAnsResult,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAnsResult {
    #[serde(rename = "ResultCode")]
    pub result_code: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub enum ClassMode {
    A,
    C,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DLMetaData {
    #[serde(rename = "DevEUI")]
    pub dev_eui: String,
    #[serde(rename = "FNSULToken", with = "hex::serde")]
    pub fns_ul_token: Token,
    #[serde(rename = "ClassMode")]
    pub class_mode: ClassMode,

    // rx windows
    #[serde(rename = "DLFreq1")]
    pub dl_freq_1: Option<f64>,
    #[serde(rename = "DataRate1")]
    pub data_rate_1: Option<u8>,
    #[serde(rename = "RXDelay1")]
    pub rx_delay_1: Option<u64>,
    #[serde(rename = "DLFreq2")]
    pub dl_freq_2: Option<f64>,
    #[serde(rename = "DataRate2")]
    pub data_rate_2: Option<u8>,
}
