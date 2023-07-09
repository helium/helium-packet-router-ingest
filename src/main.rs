use crate::config::Config;
use actions::MsgSender;
use clap::Parser;

pub type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, clap::Subcommand)]
enum Commands {
    Serve(Config),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let cli = Cli::parse();
    tracing::debug!(?cli, "opts");

    match cli.command {
        Commands::Serve(config) => run(config).await,
    }
}

pub async fn run(config: Config) {
    let http_listen_addr = config.downlink_listen;
    let grpc_listen_addr = config.uplink_listen;
    let outgoing_addr = config.lns_endpoint.clone();
    let dedup_window = config.dedupe_window;

    tracing::info!("uplink listen   :: {grpc_listen_addr}");
    tracing::info!("downlink listen :: {http_listen_addr}");
    tracing::info!("uplink post     :: {outgoing_addr}");
    tracing::info!("dedup window    :: {dedup_window}");

    let (sender, receiver) = MsgSender::new();

    let _ = tokio::try_join!(
        app::start(sender.clone(), receiver, config),
        http::start(sender.clone(), http_listen_addr),
        grpc::start(sender.clone(), grpc_listen_addr)
    );
}

mod config {
    use duration_string::DurationString;
    use std::net::SocketAddr;

    #[derive(Debug, Clone, clap::Args)]
    pub struct Config {
        /// Identity of the this NS, unique to HTTP forwarder
        #[arg(long, default_value = "6081FFFE12345678")]
        pub sender_nsid: String,
        /// Identify of the receiving NS
        #[arg(long, default_value = "0000000000000000")]
        pub receiver_nsid: String,
        /// How long were the packets held in duration time (1250ms, 1.2s)
        #[arg(long, default_value = "1250ms")]
        pub dedupe_window: DurationString,
        /// How long before Packets are cleaned out of being deduplicated in duration time.
        #[arg(long, default_value = "10s")]
        pub cleanup_window: DurationString,
        /// Helium forwarding NetID, for LNSs to identify which network to back through for downlinking.
        /// (default: C00053)
        #[arg(long, default_value = "C00053")]
        pub helium_net_id: String,
        /// NetID of the network operating the http forwarder
        #[arg(long, default_value = "000000")]
        pub target_net_id: String,
        /// Send PRStartNotif message after sending Downlink to gateway.
        /// Chirpstack does nothing with the notif message and returns an 400.
        #[arg(long, default_value = "false")]
        pub send_pr_start_notif: bool,
        /// LNS Endpoint
        #[arg(long, default_value = "http://localhost:9005")]
        pub lns_endpoint: String,
        /// Http Server for Downlinks
        #[arg(long, default_value = "0.0.0.0:9002")]
        pub downlink_listen: SocketAddr,
        /// Grpc Server for Uplinks
        #[arg(long, default_value = "0.0.0.0:9000")]
        pub uplink_listen: SocketAddr,
    }

    impl Config {
        pub async fn post(&self, body: String) {
            let res = reqwest::Client::new()
                .post(self.lns_endpoint.clone())
                .body(body.clone())
                .send()
                .await;
            tracing::info!(?body, ?res, "post");
        }
    }
}

mod actions {
    use crate::{
        grpc::{GatewayID, GatewayTx},
        packet::{PacketDown, PacketHash, PacketUp},
    };
    use tokio::sync::mpsc::{Receiver, Sender};

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
        GatewayConnect(GatewayID, GatewayTx),
        /// Gateway has Disconnected.
        GatewayDisconnect(GatewayID),
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

        pub async fn gateway_connect(&self, packet: &PacketUp, downlink_sender: GatewayTx) {
            self.0
                .send(Msg::GatewayConnect(packet.gateway_b58(), downlink_sender))
                .await
                .expect("gateway_connect");
        }

        pub async fn gateway_disconnect(&self, gateway: GatewayID) {
            self.0
                .send(Msg::GatewayDisconnect(gateway))
                .await
                .expect("gateway_disconnect");
        }

        pub async fn downlink(&self, downlink: impl Into<PacketDown>) {
            self.0
                .send(Msg::Downlink(downlink.into()))
                .await
                .expect("downlink");
        }
    }
}

mod app {
    use crate::{
        actions::{Msg, MsgSender},
        deduplicator::{Deduplicator, HandlePacket},
        grpc::{GatewayID, GatewayTx},
        http,
        packet::{PacketDown, PacketHash},
        Config,
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc::Receiver;

    pub struct App {
        deduplicator: Deduplicator,
        gateway_map: HashMap<GatewayID, GatewayTx>,
        config: Config,
        message_tx: MsgSender,
        message_rx: Receiver<Msg>,
    }

    pub enum UpdateAction {
        Noop,
        StartTimerForNewPacket(PacketHash),
        SendDownlink(GatewayTx, PacketDown, Option<String>),
        SendUplink(String),
    }

    impl App {
        pub fn new(message_tx: MsgSender, message_rx: Receiver<Msg>, config: Config) -> Self {
            Self {
                deduplicator: Deduplicator::new(),
                gateway_map: HashMap::new(),
                message_tx,
                message_rx,
                config,
            }
        }
    }

    pub fn start(
        message_tx: MsgSender,
        message_rx: Receiver<Msg>,
        config: Config,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut app = App::new(message_tx, message_rx, config);
            while let Some(msg) = app.message_rx.recv().await {
                let action = update(&mut app, msg).await;
                handle_update_action(&app, action).await;
            }
        })
    }

    pub async fn handle_update_action(app: &App, action: UpdateAction) {
        match action {
            UpdateAction::Noop => {}
            UpdateAction::StartTimerForNewPacket(hash) => {
                let dedup = app.config.dedupe_window.into();
                let cleanup = app.config.cleanup_window.into();
                let sender = app.message_tx.clone();
                tokio::spawn(async move {
                    use tokio::time::sleep;
                    sleep(dedup).await;
                    sender.uplink_send(hash.clone()).await;
                    sleep(cleanup).await;
                    sender.uplink_cleanup(hash).await;
                });
            }
            UpdateAction::SendDownlink(gw, downlink, http) => {
                let gateway_name = downlink.gateway();
                gw.send_downlink(downlink.packet_down()).await;
                tracing::info!(gw = gateway_name, "downlink sent");

                if let Some(body) = http {
                    app.config.post(body).await;
                }
            }
            UpdateAction::SendUplink(body) => app.config.post(body).await,
        }
    }

    async fn update(app: &mut App, msg: Msg) -> UpdateAction {
        match msg {
            Msg::UplinkReceive(packet) => match app.deduplicator.handle_packet(packet) {
                HandlePacket::New(hash) => UpdateAction::StartTimerForNewPacket(hash),
                HandlePacket::Existing => {
                    tracing::trace!("duplicate_packet");
                    UpdateAction::Noop
                }
            },
            Msg::UplinkSend(packet_hash) => {
                let packets = app.deduplicator.get_packets(&packet_hash);
                tracing::info!(num_packets = packets.len(), "deduplication done");
                match http::make_pr_start_req(packets, &app.config) {
                    Ok(body) => UpdateAction::SendUplink(body),
                    Err(_) => {
                        tracing::warn!("ignoring invalid packet");
                        UpdateAction::Noop
                    }
                }
            }
            Msg::UplinkCleanup(packet_hash) => {
                app.deduplicator.remove_packets(&packet_hash);
                UpdateAction::Noop
            }
            Msg::Downlink(source) => {
                let gw = source.gateway();
                let transaction_id = source.transaction_id();
                match app.gateway_map.get(&gw) {
                    Some(gateway) => match source.as_ref() {
                        PacketDown::JoinAccept(_) => {
                            let http = if app.config.send_pr_start_notif {
                                tracing::debug!("http notif");
                                let body = http::make_pr_start_notif(transaction_id, &app.config);
                                Some(body)
                            } else {
                                None
                            };
                            UpdateAction::SendDownlink(gateway.clone(), source, http)
                        }
                        PacketDown::Downlink(xmit) => {
                            let body = http::make_xmit_data_ans(xmit, &app.config);
                            tracing::debug!("http xmit ans");
                            UpdateAction::SendDownlink(gateway.clone(), source, Some(body))
                        }
                    },
                    None => {
                        tracing::warn!(?gw, "join accept for unknown gateway");
                        UpdateAction::Noop
                    }
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
}

mod packet {
    use crate::{
        http::{PRStartAnsDownlink, XmitDataReq},
        Result,
    };
    use helium_crypto::PublicKey;
    use helium_proto::services::router::{
        PacketRouterPacketDownV1, PacketRouterPacketUpV1, WindowV1,
    };
    use lorawan::parser::{DataHeader, PhyPayload, EUI64};

    #[derive(Debug, PartialEq, Eq, Hash, Clone)]
    pub struct PacketHash(pub String);

    #[derive(Debug, Clone)]
    pub struct PacketUp {
        packet: PacketRouterPacketUpV1,
        recv_time: u64,
    }

    #[derive(Debug)]
    pub enum PacketDown {
        JoinAccept(Box<PRStartAnsDownlink>),
        Downlink(Box<XmitDataReq>),
    }

    type Eui = String;
    type DevAddr = String;
    type GatewayB58 = String;

    impl PacketUp {
        pub fn gateway_b58(&self) -> GatewayB58 {
            let gateway = &self.packet.gateway;
            PublicKey::try_from(&gateway[..]).unwrap().to_string()
        }

        pub fn hash(&self) -> PacketHash {
            use sha2::{Digest, Sha256};
            PacketHash(String::from_utf8_lossy(&Sha256::digest(&self.packet.payload)).into())
        }

        pub fn routing_info(&self) -> RoutingInfo {
            let payload = &self.packet.payload;
            tracing::trace!(?payload, "payload");
            match lorawan::parser::parse(payload.clone()).expect("valid packet") {
                PhyPayload::JoinAccept(_) => RoutingInfo::Unknown,
                PhyPayload::JoinRequest(request) => {
                    let app = request.app_eui();
                    let dev = request.dev_eui();
                    RoutingInfo::eui(app, dev)
                }
                PhyPayload::Data(payload) => match payload {
                    lorawan::parser::DataPayload::Encrypted(phy) => {
                        let devaddr = phy.fhdr().dev_addr().to_string();
                        RoutingInfo::devaddr(devaddr)
                    }
                    lorawan::parser::DataPayload::Decrypted(_) => RoutingInfo::Unknown,
                },
            }
        }

        pub fn gateway_mac_str(&self) -> String {
            let hash = xxhash_rust::xxh64::xxh64(&self.packet.gateway[1..], 0);
            let hash = hash.to_be_bytes();
            hex::encode(hash)
        }

        pub fn region(&self) -> String {
            self.packet.region().to_string()
        }

        pub fn rssi(&self) -> i32 {
            self.packet.rssi
        }

        pub fn snr(&self) -> f32 {
            self.packet.snr
        }

        pub fn json_payload(&self) -> String {
            hex::encode(&self.packet.payload)
        }

        pub fn datarate_index(&self) -> u32 {
            // FIXME: handle different region
            use helium_proto::DataRate;
            match self.packet.datarate() {
                DataRate::Sf12bw125 => todo!(),
                DataRate::Sf11bw125 => todo!(),
                DataRate::Sf10bw125 => 0,
                DataRate::Sf9bw125 => 1,
                DataRate::Sf8bw125 => 2,
                DataRate::Sf7bw125 => 3,
                DataRate::Sf12bw250 => 8,
                DataRate::Sf11bw250 => 9,
                DataRate::Sf10bw250 => 10,
                DataRate::Sf9bw250 => 11,
                DataRate::Sf8bw250 => 12,
                DataRate::Sf7bw250 => 13,
                DataRate::Sf12bw500 => todo!(),
                DataRate::Sf11bw500 => todo!(),
                DataRate::Sf10bw500 => todo!(),
                DataRate::Sf9bw500 => todo!(),
                DataRate::Sf8bw500 => 4,
                DataRate::Sf7bw500 => todo!(),
                DataRate::Lrfhss1bw137 => todo!(),
                DataRate::Lrfhss2bw137 => todo!(),
                DataRate::Lrfhss1bw336 => todo!(),
                DataRate::Lrfhss2bw336 => todo!(),
                DataRate::Lrfhss1bw1523 => 5,
                DataRate::Lrfhss2bw1523 => 6,
                DataRate::Fsk50 => todo!(),
            }
        }

        pub fn frequency_mhz(&self) -> f64 {
            // Truncate -> Round -> Truncate
            let freq = (self.packet.frequency as f64) / 1_000.0;
            freq.round() / 1_000.0
        }

        /// This is the time the NS received the packet.
        pub fn recv_time(&self) -> String {
            use chrono::{DateTime, NaiveDateTime, Utc};
            let dt = DateTime::<Utc>::from_utc(
                NaiveDateTime::from_timestamp_millis(self.recv_time as i64)
                    .expect("valid timestamp"),
                Utc,
            );
            dt.to_rfc3339()
        }

        pub fn timestamp(&self) -> u64 {
            self.packet.timestamp
        }
    }

    impl From<PacketRouterPacketUpV1> for PacketUp {
        fn from(value: PacketRouterPacketUpV1) -> Self {
            use std::time::{SystemTime, UNIX_EPOCH};
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            Self {
                packet: value,
                recv_time: now,
            }
        }
    }

    impl PacketDown {
        pub fn gateway(&self) -> String {
            match self {
                Self::JoinAccept(pr_start) => pr_start.dl_meta_data.fns_ul_token.gateway.clone(),
                Self::Downlink(xmit) => xmit.dl_meta_data.fns_ul_token.gateway.clone(),
            }
        }
        pub fn transaction_id(&self) -> u64 {
            match self {
                Self::JoinAccept(pr_start) => pr_start.transaction_id,
                Self::Downlink(xmit) => xmit.transaction_id,
            }
        }

        pub fn payload(&self) -> Vec<u8> {
            hex::decode(match self {
                PacketDown::JoinAccept(inner) => inner.phy_payload.clone(),
                PacketDown::Downlink(inner) => inner.phy_payload.clone(),
            })
            .expect("hex decode downlink payload")
        }

        fn to_windows(&self) -> Result<(Option<WindowV1>, Option<WindowV1>)> {
            match (self.rx1(), self.rx2()) {
                (None, None) => anyhow::bail!("downlink contains no rx windows"),
                windows => Ok(windows),
            }
        }

        fn rx1(&self) -> Option<WindowV1> {
            match self {
                PacketDown::JoinAccept(value) => {
                    let freq1 = value.dl_meta_data.dl_freq_1;
                    let freq1 = (freq1 * 1_000_000.0) as u32; // mHz -> Hz
                    Some(WindowV1 {
                        timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 5_000_000) as u32)
                            as u64,
                        datarate: value.dl_meta_data.data_rate_1.into(),
                        frequency: freq1,
                        immediate: false,
                    })
                }
                PacketDown::Downlink(value) => {
                    let freq1 = value.dl_meta_data.dl_freq_1;
                    let freq1 = (freq1 * 1_000_000.0) as u32; // mHz -> Hz
                    Some(WindowV1 {
                        timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 5_000_000) as u32)
                            as u64,
                        datarate: value.dl_meta_data.data_rate_1.into(),
                        frequency: freq1,
                        immediate: false,
                    })
                }
            }
        }

        fn rx2(&self) -> Option<WindowV1> {
            match self {
                PacketDown::JoinAccept(value) => {
                    let freq2 = value.dl_meta_data.dl_freq_2;
                    let freq2 = (freq2 * 1_000_000.0) as u32; // mHz -> Hz
                    Some(WindowV1 {
                        timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 6_000_000) as u32)
                            as u64,
                        datarate: value.dl_meta_data.data_rate_2.into(),
                        frequency: freq2,
                        immediate: false,
                    })
                }
                PacketDown::Downlink(_value) => None,
            }
        }

        pub fn packet_down(self) -> PacketRouterPacketDownV1 {
            self.into()
        }

        pub fn as_ref(&self) -> &Self {
            self
        }
    }

    impl From<PRStartAnsDownlink> for PacketDown {
        fn from(value: PRStartAnsDownlink) -> Self {
            Self::JoinAccept(Box::new(value))
        }
    }

    impl From<XmitDataReq> for PacketDown {
        fn from(value: XmitDataReq) -> Self {
            Self::Downlink(Box::new(value))
        }
    }

    impl From<PacketDown> for PacketRouterPacketDownV1 {
        fn from(value: PacketDown) -> Self {
            let (rx1, rx2) = value.to_windows().expect("valid rx windows");
            Self {
                payload: value.payload(),
                rx1,
                rx2,
            }
        }
    }

    #[derive(Debug, PartialEq)]
    pub enum RoutingInfo {
        Eui { app: Eui, dev: Eui },
        DevAddr(DevAddr),
        Unknown,
    }

    impl RoutingInfo {
        pub fn eui(app: EUI64<&[u8]>, dev: EUI64<&[u8]>) -> Self {
            Self::Eui {
                app: EUI64::new(flip_endianness(app.as_ref()))
                    .unwrap()
                    .to_string(),
                dev: EUI64::new(flip_endianness(dev.as_ref()))
                    .unwrap()
                    .to_string(),
            }
        }
        pub fn devaddr(devaddr: DevAddr) -> Self {
            Self::DevAddr(devaddr)
        }
    }

    fn flip_endianness(slice: &[u8]) -> Vec<u8> {
        let mut flipped = Vec::with_capacity(slice.len());
        for &byte in slice.iter().rev() {
            flipped.push(byte);
        }
        flipped
    }

    #[cfg(test)]
    mod test {
        use crate::packet::RoutingInfo;
        use lorawan::parser::PhyPayload;

        #[test]
        fn eui_parse() {
            let bytes = [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 196, 160, 173, 225, 146, 91,
            ];

            if let PhyPayload::JoinRequest(request) = lorawan::parser::parse(bytes).unwrap() {
                assert_eq!(
                    RoutingInfo::Eui {
                        app: "0000000000000000".to_string(),
                        dev: "0000000000000003".to_string()
                    },
                    RoutingInfo::eui(request.app_eui(), request.dev_eui())
                );
            } else {
                assert!(false);
            }
        }
    }
}

mod deduplicator {
    use crate::packet::{PacketHash, PacketUp};
    use std::collections::HashMap;

    pub struct Deduplicator {
        packets: HashMap<PacketHash, Vec<PacketUp>>,
    }

    pub enum HandlePacket {
        New(PacketHash),
        Existing,
    }

    impl Deduplicator {
        pub fn new() -> Self {
            Self {
                packets: HashMap::new(),
            }
        }

        /// If we've never seen a packet before we will:
        /// - Insert the packet to collect the rest.
        /// - Wait for the DedupWindow, then ask for the packet to be sent.
        /// - Wait for the cleanupWindow, then remove all copies of the packet.
        pub fn handle_packet(&mut self, packet: impl Into<PacketUp>) -> HandlePacket {
            let mut result = HandlePacket::Existing;
            let packet = packet.into();
            let hash = packet.hash();
            self.packets
                .entry(hash.clone())
                .and_modify(|bucket| bucket.push(packet.clone()))
                .or_insert_with(|| {
                    result = HandlePacket::New(hash);
                    vec![packet]
                });
            result
        }

        pub fn get_packets(&self, hash: &PacketHash) -> Vec<PacketUp> {
            self.packets
                .get(hash)
                .expect("packets exist for hash")
                .to_owned()
        }

        pub fn remove_packets(&mut self, hash: &PacketHash) {
            self.packets.remove(hash);
        }
    }
}

mod http {
    use crate::{
        actions::MsgSender,
        grpc::GatewayID,
        packet::{PacketUp, RoutingInfo},
        Config, Result,
    };
    use axum::{
        extract,
        response::IntoResponse,
        routing::{get, post, Router},
        Extension,
    };
    use hex::FromHex;
    use reqwest::StatusCode;
    use std::{collections::HashMap, net::SocketAddr};
    use tracing::instrument;

    type TransactionID = u64;

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

    pub fn make_pr_start_notif(transaction_id: TransactionID, http_config: &Config) -> String {
        serde_json::to_string(&serde_json::json!({
            "ProtocolVersion": "1.1",
            "SenderID": http_config.helium_net_id,
            "ReceiverID": http_config.target_net_id,
            "TransactionID": transaction_id,
            "MessageType": "PRStartNotif",
            "SenderNSID": http_config.sender_nsid,
            "ReceiverNSID": http_config.receiver_nsid,
            "Result": {"ResultCode": "Success"}
        }))
        .expect("pr_start_notif json")
    }

    pub fn make_xmit_data_ans(xmit: &XmitDataReq, http_config: &Config) -> String {
        serde_json::to_string(&serde_json::json!({
            "ProtocolVersion": "1.1",
            "MessageType": "XmitDataAns",
            "SenderID": http_config.helium_net_id,
            "ReceiverID": http_config.target_net_id,
            "SenderNSID": http_config.sender_nsid,
            "ReceiverNSID": http_config.receiver_nsid,
            "TransactionID": xmit.transaction_id,
            "Result": {"ResultCode": "Success"},
            "DLFreq1": xmit.dl_meta_data.dl_freq_1
        }))
        .expect("xmit_data_ans json")
    }

    pub fn make_pr_start_req(packets: Vec<PacketUp>, config: &Config) -> Result<String> {
        let packet = packets.first().expect("at least one packet");

        let (routing_key, routing_value) = match packet.routing_info() {
            RoutingInfo::Eui { dev, .. } => ("DevEUI", dev),
            RoutingInfo::DevAddr(devaddr) => ("DevAddr", devaddr),
            RoutingInfo::Unknown => todo!("should never get here"),
        };

        let mut gw_info = vec![];
        for packet in packets.iter() {
            gw_info.push(serde_json::json!({
                "ID": packet.gateway_mac_str(),
                "RFRegion": packet.region(),
                "RSSI": packet.rssi(),
                "SNR": packet.snr(),
                "DLAllowed": true
            }));
        }

        Ok(serde_json::to_string(&serde_json::json!({
            "ProtocolVersion" : "1.1",
            "MessageType": "PRStartReq",
            "SenderNSID": config.sender_nsid,
            "ReceiverNSID": config.receiver_nsid,
            "DedupWindowSize": config.dedupe_window.to_string(),
            "SenderID": config.helium_net_id,
            "ReceiverID": config.target_net_id,
            "PHYPayload": packet.json_payload(),
            "ULMetaData": {
                routing_key: routing_value,
                "DataRate": packet.datarate_index(),
                "ULFreq": packet.frequency_mhz(),
                "RecvTime": packet.recv_time(),
                "RFRegion": packet.region(),
                "FNSULToken": make_token(packet.gateway_b58(), packet.timestamp()),
                "GWCnt": packets.len(),
                "GWInfo": gw_info
            }
        }))
        .expect("pr_start_req json"))
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
    pub struct Token {
        pub timestamp: u64,
        pub gateway: GatewayID,
    }

    impl FromHex for Token {
        type Error = anyhow::Error;

        fn from_hex<T: AsRef<[u8]>>(hex: T) -> std::result::Result<Self, Self::Error> {
            let s = hex::decode(hex).unwrap();
            Ok(serde_json::from_slice(&s[..]).unwrap())
        }
    }

    fn make_token(gateway: GatewayID, timestamp: u64) -> String {
        let token = Token { gateway, timestamp };
        hex::encode(serde_json::to_string(&token).unwrap())
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
    pub struct DLMetaData {
        #[serde(rename = "DevEUI")]
        pub dev_eui: String,
        #[serde(rename = "DLFreq1")]
        pub dl_freq_1: f32,
        #[serde(rename = "DataRate1")]
        pub data_rate_1: u8,
        #[serde(rename = "RXDelay1")]
        pub rx_delay_1: u8,
        #[serde(rename = "FNSULToken", with = "hex::serde")]
        pub fns_ul_token: Token,
        #[serde(rename = "ClassMode")]
        pub class_mode: String,
        #[serde(rename = "HiPriorityFlag")]
        pub high_priority: bool,
        #[serde(rename = "GWInfo")]
        pub gw_info: Vec<GWInfo>,
    }

    #[derive(Debug, Clone, serde::Deserialize)]
    pub struct GWInfo {
        #[serde(rename = "ULToken")]
        pub ul_token: Option<String>,
    }

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
    pub struct PRStartAnsDownlink {
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
        #[serde(rename = "DevEUI")]
        pub dev_eui: String,
        #[serde(rename = "FCntUp")]
        pub f_cnt_up: u32,
        #[serde(rename = "DLMetaData")]
        pub dl_meta_data: PRStartAnsDLMetaData,
        #[serde(rename = "DevAddr")]
        pub dev_addr: String,
    }

    #[derive(Debug, Clone, serde::Deserialize)]
    pub struct PRStartAnsResult {
        #[serde(rename = "ResultCode")]
        pub result_code: String,
    }

    #[derive(Debug, Clone, serde::Deserialize)]
    pub struct PRStartAnsDLMetaData {
        #[serde(rename = "DevEUI")]
        pub dev_eui: String,
        #[serde(rename = "FPort")]
        pub f_port: Option<String>,
        #[serde(rename = "FCntDown")]
        pub f_cnt_down: Option<String>,
        #[serde(rename = "Confirmed")]
        pub confirmed: bool,
        #[serde(rename = "DLFreq1")]
        pub dl_freq_1: f32,
        #[serde(rename = "DLFreq2")]
        pub dl_freq_2: f32,
        #[serde(rename = "RXDelay1")]
        pub rx_delay_1: u8,
        #[serde(rename = "ClassMode")]
        pub class_mode: String,
        #[serde(rename = "DataRate1")]
        pub data_rate_1: u8,
        #[serde(rename = "DataRate2")]
        pub data_rate_2: u8,
        #[serde(rename = "FNSULToken", with = "hex::serde")]
        pub fns_ul_token: Token,
        #[serde(rename = "GWInfo")]
        pub gw_info: Vec<HashMap<String, Option<String>>>,
        #[serde(rename = "HiPriorityFlag")]
        pub hi_priority_flag: bool,
    }

    #[cfg(test)]
    mod test {
        use super::PRStartAnsPlain;
        use crate::{
            http::{PRStartAnsDownlink, XmitDataReq},
            packet::PacketDown,
        };
        use helium_proto::services::router::PacketRouterPacketDownV1;

        #[test]
        fn xmit_data_req_to_packet_down_v1() {
            let value = serde_json::json!({
                "ProtocolVersion":"1.0",
                "SenderID":"000024",
                "ReceiverID":"c00053",
                "TransactionID":274631693,
                "MessageType":"XmitDataReq",
                "PHYPayload":"6073000048ab00000300020070030000ff01063d32ce60",
                "ULMetaData":null,
                "DLMetaData":{
                    "DevEUI":"0000000000000003",
                    "FPort":null,
                    "FCntDown":null,
                    "Confirmed":false,
                    "DLFreq1":926.9,
                    "DLFreq2":923.3,
                    "RXDelay1":1,
                    "ClassMode":"A",
                    "DataRate1":10,
                    "DataRate2":8,
                    "FNSULToken":"7b2274696d657374616d70223a31303739333532372c2267617465776179223a2231336a6e776e5a594c446777394b64347a7033336379783474424e514a346a4d6f4e76485469467976556b41676b6851557a39227d",
                    "GWInfo":[{
                        "FineRecvTime":null,
                        "RSSI":null,
                        "SNR":null,
                        "Lat":null,
                        "Lon":null,
                        "DLAllowed":null
                    }],
                    "HiPriorityFlag":false}
                }
            );
            let xmit: XmitDataReq = serde_json::from_value(value).unwrap();
            println!("xmit: {xmit:#?}");
            let packet: PacketDown = xmit.into();
            let down: PacketRouterPacketDownV1 = packet.into();
            println!("down: {down:#?}");
        }

        #[test]
        fn join_accept_to_packet_down_v1() {
            let value = serde_json::json!( {
                "ProtocolVersion": "1.1",
                "SenderID": "000024",
                "ReceiverID": "c00053",
                "TransactionID":  193858937,
                "MessageType": "PRStartAns",
                "Result": {"ResultCode": "Success"},
                "PHYPayload": "209c7848d681b589da4b8e5544460a693383bb7e5d47b849ef0d290bafb20872ae",
                "DevEUI": "0000000000000003",
                "Lifetime": null,
                "FNwkSIntKey": null,
                "NwkSKey": {
                    "KEKLabel": "",
                    "AESKey": "0e2baf26327308e63afe62be15edea6a"
                },
                "FCntUp": 0,
                "ServiceProfile": null,
                "DLMetaData": {
                    "DevEUI": "0000000000000003",
                    "FPort": null,
                    "FCntDown": null,
                    "Confirmed": false,
                    "DLFreq1": 925.1,
                    "DLFreq2": 923.3,
                    "RXDelay1": 5,
                    "ClassMode": "A",
                    "DataRate1": 10,
                    "DataRate2": 8,
                    "FNSULToken": "7b2274696d657374616d70223a343034383533323435322c2267617465776179223a2231336a6e776e5a594c446777394b64347a7033336379783474424e514a346a4d6f4e76485469467976556b41676b6851557a39227d",
                    "GWInfo": [{"FineRecvTime": null,"RSSI": null,"SNR": null,"Lat": null,"Lon": null,"DLAllowed": null}],
                  "HiPriorityFlag": false
                },
                "DevAddr": "48000037"
            });
            let pr_start: PRStartAnsDownlink =
                serde_json::from_value(value).expect("to packet down");
            println!("packet: {pr_start:#?}");
            let packet: PacketDown = pr_start.into();
            let down: PacketRouterPacketDownV1 = packet.into();
            println!("down: {down:#?}");
        }

        #[test]
        fn mic_failed() {
            let value = serde_json::json!({
                "ProtocolVersion": "1.1",
                "SenderID": "000024",
                "ReceiverID": "c00053",
                "TransactionID": 517448448,
                "MessageType": "PRStartAns",
                "Result": {
                    "ResultCode": "MICFailed",
                    "Description": "Invalid MIC"
                },
                "Lifetime": null,
                "FNwkSIntKey": null,
                "NwkSKey": null,
                "FCntUp": null,
                "ServiceProfile": null,
                "DLMetaData": null
            });
            let packet: PRStartAnsPlain = serde_json::from_value(value).expect("to pr plain");
            println!("plain: {packet:#?}");
        }
    }
}

mod grpc {
    use std::net::SocketAddr;

    use crate::{actions::MsgSender, packet::PacketUp, Result};
    use helium_proto::services::router::{
        envelope_down_v1, envelope_up_v1, packet_server::Packet, packet_server::PacketServer,
        EnvelopeDownV1, EnvelopeUpV1, PacketRouterPacketDownV1,
    };
    use tokio::sync::mpsc::Sender;
    use tokio_stream::wrappers::ReceiverStream;
    use tonic::{Request, Response, Status, Streaming};
    use tracing::instrument;

    #[instrument]
    pub fn start(sender: MsgSender, addr: SocketAddr) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(PacketServer::new(Gateways::new(sender)))
                .serve(addr)
                .await
                .unwrap();
        })
    }

    #[derive(Debug, Clone)]
    pub struct GatewayTx(pub Sender<Result<EnvelopeDownV1, Status>>);
    pub type GatewayID = String;

    impl GatewayTx {
        pub async fn send_downlink(&self, downlink: impl Into<PacketRouterPacketDownV1>) {
            let _ = self
                .0
                .send(Ok(EnvelopeDownV1 {
                    data: Some(envelope_down_v1::Data::Packet(downlink.into())),
                }))
                .await;
        }
    }

    #[derive(Debug)]
    struct Gateways {
        sender: MsgSender,
    }

    impl Gateways {
        pub fn new(sender: MsgSender) -> Self {
            Self { sender }
        }
    }

    #[tonic::async_trait]
    impl Packet for Gateways {
        type routeStream = ReceiverStream<Result<EnvelopeDownV1, Status>>;

        async fn route(
            &self,
            request: Request<Streaming<EnvelopeUpV1>>,
        ) -> Result<Response<Self::routeStream>, Status> {
            let sender = self.sender.clone();
            let mut req = request.into_inner();

            let (downlink_sender, downlink_receiver) = tokio::sync::mpsc::channel(128);
            tracing::info!("connection");

            tokio::spawn(async move {
                let mut gateway_b58 = None;
                while let Ok(Some(msg)) = req.message().await {
                    if let Some(envelope_up_v1::Data::Packet(packet)) = msg.data {
                        let packet: PacketUp = packet.into();
                        if gateway_b58.is_none() {
                            sender
                                .gateway_connect(&packet, GatewayTx(downlink_sender.clone()))
                                .await;
                            gateway_b58 = Some(packet.gateway_b58())
                        }
                        sender.uplink_receive(packet).await;
                    } else {
                        tracing::warn!(?msg.data, "ignoring message");
                    }
                }

                tracing::info!("gateway went down");
                sender
                    .gateway_disconnect(gateway_b58.expect("packet was sent by gateway"))
                    .await;
            });

            Ok(Response::new(ReceiverStream::new(downlink_receiver)))
        }
    }
}
